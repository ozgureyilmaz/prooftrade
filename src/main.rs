use std::error::Error;
use std::io::{BufRead, Write};
use std::net::TcpListener;
use std::path::PathBuf;

use prooftrade::agent_contract::AgentToolRegistry;
use prooftrade::demo::run_simulation_demo;
use prooftrade::domain::TradingUniverse;
use prooftrade::operator::{OperatorController, OperatorProjector, OperatorReadModel};
use prooftrade::persistence::AuditStore;
use prooftrade::risk::SafetyStateStore;
use serde_json::{Value, json};
use uuid::Uuid;

fn main() {
    if let Err(error) = run() {
        eprintln!("prooftrade: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("simulation") | Some("demo") => run_simulation(args.collect()),
        Some("operator") => run_operator(args.collect()),
        Some("agent-mcp") => run_agent_mcp(args.collect()),
        Some("replay") => run_replay(args.collect()),
        Some("--help") | None => {
            print_help();
            Ok(())
        }
        Some(command) => Err(format!("unknown command: {command}; use --help").into()),
    }
}

fn run_simulation(arguments: Vec<String>) -> Result<(), Box<dyn Error>> {
    let database =
        flag_path(&arguments, "--db").unwrap_or_else(|| PathBuf::from("prooftrade-demo.sqlite"));
    let receipt = flag_path(&arguments, "--output")
        .unwrap_or_else(|| PathBuf::from("prooftrade-demo-receipt.json"));
    if let Some(cash) = flag_value(&arguments, "--cash")
        && cash != "30"
    {
        return Err("the canonical demo scenario uses exactly 30 USDT of synthetic cash".into());
    }
    if let Some(scenario) = flag_value(&arguments, "--scenario")
        && scenario != "btc-entry-and-hard-take-profit"
    {
        return Err("unknown scenario; use btc-entry-and-hard-take-profit".into());
    }
    let result = run_simulation_demo(Some(&database), Some(&receipt))?;
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}

fn run_operator(arguments: Vec<String>) -> Result<(), Box<dyn Error>> {
    let database =
        flag_path(&arguments, "--db").unwrap_or_else(|| PathBuf::from("prooftrade-demo.sqlite"));
    let bind = flag_value(&arguments, "--bind").unwrap_or_else(|| "127.0.0.1:8787".to_owned());
    if !(bind.starts_with("127.0.0.1:") || bind.starts_with("[::1]:")) {
        return Err("operator host is intentionally loopback-only".into());
    }

    let projector =
        OperatorProjector::new(AuditStore::open(&database)?, OperatorReadModel::default());
    let _ = projector.rehydrate()?;
    let model = projector.snapshot();
    let controller =
        OperatorController::with_projector(projector, SafetyStateStore::new(model.safety_state));
    let listener = TcpListener::bind(&bind)?;
    println!("ProofTrade operator dashboard: http://{bind}/");
    println!("Read model: http://{bind}/api/read-model");
    println!("Press Ctrl-C to stop; KILL requires POST body exactly KILL.");
    if arguments.iter().any(|argument| argument == "--once") {
        controller.serve_once(listener, &model)?;
    } else {
        controller.serve(listener)?;
    }
    Ok(())
}

fn run_agent_mcp(arguments: Vec<String>) -> Result<(), Box<dyn Error>> {
    let database =
        flag_path(&arguments, "--db").unwrap_or_else(|| PathBuf::from("prooftrade-demo.sqlite"));
    let projector =
        OperatorProjector::new(AuditStore::open(&database)?, OperatorReadModel::default());
    let _ = projector.rehydrate()?;
    let registry = AgentToolRegistry::new(TradingUniverse::initial(), projector.state());
    let stdin = std::io::stdin();
    let mut stdout = std::io::BufWriter::new(std::io::stdout());
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let request: Value = serde_json::from_str(&line)?;
        if request.get("method").and_then(Value::as_str) == Some("notifications/initialized") {
            continue;
        }
        let response = handle_mcp_request(&registry, &request);
        writeln!(stdout, "{}", serde_json::to_string(&response)?)?;
        stdout.flush()?;
    }
    Ok(())
}

fn handle_mcp_request(registry: &AgentToolRegistry, request: &Value) -> Value {
    let id = request.get("id").cloned().unwrap_or(Value::Null);
    let method = request
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or_default();
    match method {
        "initialize" => json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "protocolVersion": "2024-11-05",
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "prooftrade-agent", "version": env!("CARGO_PKG_VERSION")}
            }
        }),
        "ping" => json!({"jsonrpc": "2.0", "id": id, "result": {}}),
        "tools/list" => json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {"tools": registry.list()}
        }),
        "tools/call" => {
            let name = request
                .pointer("/params/name")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let arguments = request
                .pointer("/params/arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            match registry.call(name, arguments) {
                Ok(value) => json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {"content": [{"type": "text", "text": value.to_string()}], "isError": false}
                }),
                Err(error) => json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {"content": [{"type": "text", "text": error.to_string()}], "isError": true}
                }),
            }
        }
        _ => json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {"code": -32601, "message": "method not found"}
        }),
    }
}

fn run_replay(arguments: Vec<String>) -> Result<(), Box<dyn Error>> {
    let database = flag_path(&arguments, "--db").ok_or("replay requires --db <sqlite-path>")?;
    let entity = flag_value(&arguments, "--entity")
        .ok_or("replay requires --entity <uuid>")?
        .parse::<Uuid>()?;
    let store = AuditStore::open(database)?;
    println!("{}", store.replay(entity)?.serialized());
    Ok(())
}

fn flag_value(arguments: &[String], flag: &str) -> Option<String> {
    arguments
        .windows(2)
        .find(|pair| pair[0] == flag)
        .map(|pair| pair[1].clone())
}

fn flag_path(arguments: &[String], flag: &str) -> Option<PathBuf> {
    flag_value(arguments, flag).map(PathBuf::from)
}

fn print_help() {
    println!(
        "ProofTrade\n\nCommands:\n  prooftrade simulation [--cash 30] [--db PATH] [--output PATH]\n  prooftrade operator [--db PATH] [--bind 127.0.0.1:8787] [--once]\n  prooftrade agent-mcp [--db PATH]\n  prooftrade replay --db PATH --entity UUID\n\nThe simulation command is synthetic-only and never invokes ATK or Hermes."
    );
}
