use std::process::Command;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use prooftrade::atk::{McpError, McpTransport, StdioTransport};
use serde_json::json;

#[test]
fn stdio_transport_correlates_out_of_order_responses_and_ignores_unknown_ids() {
    let mut command = Command::new("sh");
    command.arg("-c").arg(
        r#"
while IFS= read -r line; do
  case "$line" in
    *one*) (printf '%s\n' '{"jsonrpc":"2.0","id":999,"result":{"value":"unknown"}}'; sleep 0.05; printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"value":"one"}}') & ;;
    *two*) printf '%s\n' '{"jsonrpc":"2.0","id":2,"result":{"value":"two"}}' ;;
  esac
done
wait
"#,
    );
    let transport = Arc::new(StdioTransport::spawn(&mut command).unwrap());
    let first_transport = Arc::clone(&transport);
    let first =
        thread::spawn(move || first_transport.request("one", json!({}), Duration::from_secs(1)));
    thread::sleep(Duration::from_millis(10));
    let second = transport
        .request("two", json!({}), Duration::from_secs(1))
        .unwrap();

    assert_eq!(second["result"]["value"], "two");
    assert_eq!(first.join().unwrap().unwrap()["result"]["value"], "one");
}

#[test]
fn stdio_transport_reports_malformed_json_without_treating_it_as_success() {
    let mut command = Command::new("sh");
    command
        .arg("-c")
        .arg("IFS= read -r line; printf '%s\\n' 'not-json'; sleep 1");
    let transport = StdioTransport::spawn(&mut command).unwrap();

    let error = transport
        .request("one", json!({}), Duration::from_secs(1))
        .unwrap_err();

    assert!(matches!(error, McpError::MalformedJson(_)));
}

#[test]
fn stdio_transport_reports_process_death_and_marks_it_unavailable() {
    let mut command = Command::new("sh");
    command.arg("-c").arg("exit 0");
    let transport = StdioTransport::spawn(&mut command).unwrap();
    for _ in 0..100 {
        if !transport.is_alive() {
            break;
        }
        thread::sleep(Duration::from_millis(1));
    }

    assert!(!transport.is_alive());
    let error = transport
        .request("one", json!({}), Duration::from_millis(10))
        .unwrap_err();
    assert_eq!(error, McpError::ProcessExited);
}
