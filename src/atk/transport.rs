use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use serde_json::{Value, json};

use super::error::McpError;

pub trait McpTransport: Send + Sync {
    fn request(&self, method: &str, params: Value, timeout: Duration) -> Result<Value, McpError>;

    fn notify(&self, method: &str, params: Value) -> Result<(), McpError>;

    fn is_alive(&self) -> bool;

    fn shutdown(&self) -> Result<(), McpError>;
}

struct SharedProcess {
    stdin: Mutex<std::process::ChildStdin>,
    pending: Mutex<HashMap<u64, Sender<Result<Value, McpError>>>>,
    alive: AtomicBool,
}

pub struct StdioTransport {
    child: Mutex<Option<Child>>,
    shared: Arc<SharedProcess>,
    next_id: AtomicU64,
}

impl StdioTransport {
    pub fn spawn(command: &mut Command) -> Result<Self, McpError> {
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| McpError::Io(error.to_string()))?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| McpError::Io("MCP child stdin was not piped".to_owned()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| McpError::Io("MCP child stdout was not piped".to_owned()))?;
        if let Some(stderr) = child.stderr.take() {
            thread::spawn(move || {
                let mut reader = BufReader::new(stderr);
                let mut line = String::new();
                while reader.read_line(&mut line).unwrap_or(0) > 0 {
                    line.clear();
                }
            });
        }

        let shared = Arc::new(SharedProcess {
            stdin: Mutex::new(stdin),
            pending: Mutex::new(HashMap::new()),
            alive: AtomicBool::new(true),
        });
        Self::start_reader(stdout, Arc::clone(&shared));

        Ok(Self {
            child: Mutex::new(Some(child)),
            shared,
            next_id: AtomicU64::new(1),
        })
    }

    fn start_reader(stdout: std::process::ChildStdout, shared: Arc<SharedProcess>) {
        thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            let mut line = String::new();
            loop {
                line.clear();
                match reader.read_line(&mut line) {
                    Ok(0) => break,
                    Ok(_) => {
                        let value = match serde_json::from_str::<Value>(line.trim()) {
                            Ok(value) => value,
                            Err(error) => {
                                Self::fail_pending(
                                    &shared,
                                    McpError::MalformedJson(error.to_string()),
                                );
                                continue;
                            }
                        };
                        let Some(id) = value.get("id").and_then(Value::as_u64) else {
                            continue;
                        };
                        let sender = shared
                            .pending
                            .lock()
                            .ok()
                            .and_then(|mut pending| pending.remove(&id));
                        if let Some(sender) = sender {
                            let _ = sender.send(Ok(value));
                        }
                    }
                    Err(error) => {
                        Self::fail_pending(&shared, McpError::Io(error.to_string()));
                        break;
                    }
                }
            }
            shared.alive.store(false, Ordering::SeqCst);
            Self::fail_pending(&shared, McpError::ProcessExited);
        });
    }

    fn fail_pending(shared: &SharedProcess, error: McpError) {
        if let Ok(mut pending) = shared.pending.lock() {
            for (_, sender) in pending.drain() {
                let _ = sender.send(Err(error.clone()));
            }
        }
    }

    fn write_line(&self, value: &Value) -> Result<(), McpError> {
        if !self.is_alive() {
            return Err(McpError::ProcessExited);
        }
        let line = serde_json::to_string(value).map_err(|error| McpError::Io(error.to_string()))?;
        let mut stdin = self
            .shared
            .stdin
            .lock()
            .map_err(|_| McpError::Io("MCP stdin lock poisoned".to_owned()))?;
        stdin
            .write_all(line.as_bytes())
            .and_then(|_| stdin.write_all(b"\n"))
            .and_then(|_| stdin.flush())
            .map_err(|error| {
                self.shared.alive.store(false, Ordering::SeqCst);
                McpError::Io(error.to_string())
            })
    }
}

impl McpTransport for StdioTransport {
    fn request(&self, method: &str, params: Value, timeout: Duration) -> Result<Value, McpError> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (sender, receiver) = mpsc::channel();
        self.shared
            .pending
            .lock()
            .map_err(|_| McpError::Io("MCP pending-request lock poisoned".to_owned()))?
            .insert(id, sender);

        let request = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        if let Err(error) = self.write_line(&request) {
            if let Ok(mut pending) = self.shared.pending.lock() {
                pending.remove(&id);
            }
            return Err(error);
        }

        match receiver.recv_timeout(timeout) {
            Ok(result) => result,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if let Ok(mut pending) = self.shared.pending.lock() {
                    pending.remove(&id);
                }
                Err(McpError::RequestTimeout {
                    method: method.to_owned(),
                })
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(McpError::ProcessExited),
        }
    }

    fn notify(&self, method: &str, params: Value) -> Result<(), McpError> {
        self.write_line(&json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        }))
    }

    fn is_alive(&self) -> bool {
        self.shared.alive.load(Ordering::SeqCst)
    }

    fn shutdown(&self) -> Result<(), McpError> {
        if !self.shared.alive.swap(false, Ordering::SeqCst) {
            return Ok(());
        }
        Self::fail_pending(&self.shared, McpError::ProcessExited);
        let mut child = self
            .child
            .lock()
            .map_err(|_| McpError::Io("MCP child lock poisoned".to_owned()))?;
        if let Some(child) = child.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
        *child = None;
        Ok(())
    }
}

impl Drop for StdioTransport {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}
