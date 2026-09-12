use std::env;
use std::net::TcpListener;
use std::path::PathBuf;

use prooftrade::operator::{OperatorController, OperatorProjector, OperatorReadModel};
use prooftrade::persistence::AuditStore;
use prooftrade::risk::SafetyStateStore;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let database = env::var_os("PROOFTRADE_OPERATOR_DB")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("prooftrade.sqlite"));
    let bind = env::var("PROOFTRADE_OPERATOR_BIND").unwrap_or_else(|_| "127.0.0.1:3030".to_owned());
    if !(bind.starts_with("127.0.0.1:") || bind.starts_with("[::1]:")) {
        return Err("operator host is intentionally loopback-only".into());
    }
    let projector =
        OperatorProjector::new(AuditStore::open(&database)?, OperatorReadModel::default());
    let _ = projector.rehydrate()?;
    let safety_state = projector.snapshot().safety_state;
    let controller =
        OperatorController::with_projector(projector, SafetyStateStore::new(safety_state));
    let listener = TcpListener::bind(&bind)?;
    eprintln!("ProofTrade operator listening on http://{bind}");
    eprintln!("database: {}", database.display());
    controller.serve(listener)?;
    Ok(())
}
