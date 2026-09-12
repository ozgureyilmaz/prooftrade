//! Deterministic, reviewable end-to-end SIMULATION demo.
//!
//! The demo deliberately stops at the synthetic execution boundary. It does
//! not construct an ATK client, invoke Hermes, read credentials, or make a
//! network call.

use std::path::{Path, PathBuf};

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::json;
use thiserror::Error;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::domain::{DecisionId, Instrument, TradeDecision, Urgency};
use crate::execution::{
    ExecutionError, ExecutionPlan, ExecutionPolicy, ExecutionReceipt, ExecutionState, MarketQuote,
    SimulationExecutor,
};
use crate::operator::{OperatorProjector, OperatorReadModel, PortfolioView};
use crate::persistence::{AuditEvent, AuditStore, FillRecord, RunId, StorageError};
use crate::reporting::ReportBuilder;
use crate::risk::{
    AccountState, OperatingMode, RiskConfig, RiskEngine, RiskError, RiskSnapshot, SafetyState,
};
use crate::simulation::{
    FeeModel, FillModel, MarketSnapshot, SimulationConfig, SimulationEngine, SimulationError,
    SimulationFill, SlippageModel,
};

#[derive(Debug, Error)]
pub enum DemoError {
    #[error("demo simulation failed: {0}")]
    Simulation(#[from] SimulationError),
    #[error("demo execution failed: {0}")]
    Execution(#[from] ExecutionError),
    #[error("demo storage failed: {0}")]
    Storage(#[from] StorageError),
    #[error("demo risk evaluation failed: {0}")]
    Risk(#[from] RiskError),
    #[error("demo serialization failed: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("demo operator projection failed: {0}")]
    Operator(String),
    #[error("demo filesystem operation failed: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DemoReceipt {
    pub schema_version: u16,
    pub scenario: String,
    pub fixture_label: String,
    pub environment: String,
    pub run_id: String,
    pub decision_id: String,
    pub risk_decision_id: String,
    pub execution_id: String,
    pub audit_event_count: usize,
    pub replay_event_count: usize,
    pub operator_revision: u64,
    pub report: serde_json::Value,
}

const DEMO_RUN_UUID: Uuid = Uuid::from_u128(0x50524f4f46545241444544454d4f3031);
const DEMO_DECISION_UUID: Uuid = Uuid::from_u128(0x50524f4f4654524144454445433031);
const DEMO_RISK_UUID: Uuid = Uuid::from_u128(0x50524f4f46545241444552534b303031);
const DEMO_EXECUTION_UUID: Uuid = Uuid::from_u128(0x50524f4f465452414445455845433031);
const DEMO_INITIAL_FILL_UUID: Uuid = Uuid::from_u128(0x50524f4f46545241444546494c4c3031);
const DEMO_PROTECTIVE_FILL_UUID: Uuid = Uuid::from_u128(0x50524f4f46545241444546494c4c3032);

const DEMO_SCENARIO: &str = "btc-entry-and-hard-take-profit";
const DEMO_FIXTURE_LABEL: &str = "deterministic-fixture";

pub fn run_simulation_demo(
    database_path: Option<&Path>,
    receipt_path: Option<&Path>,
) -> Result<DemoReceipt, DemoError> {
    let run_id = RunId::from_uuid(DEMO_RUN_UUID);
    let directory = std::env::temp_dir().join(format!("prooftrade-demo-{}", Uuid::new_v4()));
    let temporary_database = database_path.is_none();
    let database_path = database_path
        .map(PathBuf::from)
        .unwrap_or_else(|| directory.join("run.sqlite"));
    if temporary_database {
        std::fs::create_dir_all(&directory)?;
    } else if let Some(parent) = database_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent)?;
    }

    let result = run_simulation_demo_inner(run_id, &database_path);
    let result = match result {
        Ok(receipt) => {
            if let Some(receipt_path) = receipt_path {
                if let Some(parent) = receipt_path
                    .parent()
                    .filter(|parent| !parent.as_os_str().is_empty())
                {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(receipt_path, serde_json::to_vec_pretty(&receipt)?)?;
            }
            Ok(receipt)
        }
        Err(error) => Err(error),
    };
    if temporary_database {
        let _ = std::fs::remove_dir_all(directory);
    }
    result
}

fn run_simulation_demo_inner(
    run_id: RunId,
    database_path: &Path,
) -> Result<DemoReceipt, DemoError> {
    let instrument =
        Instrument::parse("BTC-USDT").map_err(|error| DemoError::Operator(error.to_string()))?;
    let entry_time = OffsetDateTime::from_unix_timestamp(1_700_000_000)
        .map_err(|error| DemoError::Operator(error.to_string()))?;
    let exit_time = OffsetDateTime::from_unix_timestamp(1_700_000_060)
        .map_err(|error| DemoError::Operator(error.to_string()))?;
    let initial_market = MarketSnapshot::new(
        instrument.clone(),
        entry_time,
        Decimal::from(99),
        Decimal::from(100),
    )?;
    let protective_market = MarketSnapshot::new(
        instrument.clone(),
        exit_time,
        Decimal::from(131),
        Decimal::from(132),
    )?;

    let mut decision = TradeDecision::buy(
        DecisionId::from_uuid(DEMO_DECISION_UUID),
        instrument.clone(),
        entry_time,
        Decimal::from(10),
        Decimal::new(33, 2),
    );
    decision.urgency = Urgency::High;
    let account = AccountState::new(Decimal::from(30), Decimal::from(30), Decimal::from(30))
        .map_err(|error| DemoError::Operator(error.to_string()))?;
    let snapshot = RiskSnapshot::new(
        entry_time,
        account,
        Vec::new(),
        OperatingMode::Simulation,
        SafetyState::Normal,
    )
    .map_err(|error| DemoError::Operator(error.to_string()))?;
    let mut risk = RiskEngine::new(RiskConfig::default()).evaluate(&decision, &snapshot);
    risk.risk_decision_id = DEMO_RISK_UUID;
    if !risk.is_approved() {
        return Err(DemoError::Operator(format!(
            "deterministic demo intent was rejected: {:?}",
            risk.reasons
        )));
    }
    let quote = MarketQuote::new(
        instrument.clone(),
        entry_time,
        Decimal::from(99),
        Decimal::from(100),
    )?;
    let mut plan = ExecutionPolicy::default().plan(&risk, &decision, &quote)?;
    plan.execution_id = DEMO_EXECUTION_UUID;
    plan.client_order_id = "pt-demo-entry-0001".to_owned();

    let mut store = AuditStore::open(database_path)?;
    store.append_event(AuditEvent::new(
        run_id,
        "demo_started",
        run_id.as_uuid().to_string(),
        entry_time,
        json!({
            "scenario": DEMO_SCENARIO,
            "fixture_label": DEMO_FIXTURE_LABEL,
            "environment": "SIMULATION",
            "network_calls": false,
            "atk_calls": false,
        }),
    ))?;
    store.save_decision_bundle(run_id, &decision, &risk)?;

    let config = SimulationConfig::new(Decimal::from(30))
        .with_fee_model(FeeModel::FixedRate(Decimal::new(1, 3)))
        .with_slippage_model(SlippageModel::FixedBps(Decimal::from(5)))
        .with_fill_model(FillModel::Full);
    let mut executor = SimulationExecutor::new(SimulationEngine::new(config)?);
    let entry = executor.execute(&plan, initial_market.clone())?;
    let entry_fill = entry
        .fill
        .clone()
        .ok_or_else(|| DemoError::Operator("entry fixture did not fill".to_owned()))?;
    let entry_receipt = receipt_from_fill(&plan, &entry_fill);
    store.save_execution(run_id, &plan, &entry_receipt)?;
    store.save_fill(run_id, &fill_record(&entry_fill, DEMO_INITIAL_FILL_UUID))?;

    let entry_position = executor
        .engine()
        .position(&instrument)
        .cloned()
        .ok_or_else(|| DemoError::Operator("entry fixture did not create inventory".to_owned()))?;
    let mut report = ReportBuilder::new(Decimal::from(30));
    report.record_equity(executor.engine().portfolio_snapshot()?.equity);
    report.record_trade(Decimal::ZERO, entry_fill.fee);
    report.record_slippage(entry_fill.slippage);

    let protective = executor
        .engine_mut()
        .on_market_snapshot(protective_market)?;
    let protective_fill = protective
        .iter()
        .find_map(|result| result.fill.clone())
        .ok_or_else(|| DemoError::Operator("protective fixture did not fill".to_owned()))?;
    let realized = protective_fill.gross_notional
        - protective_fill.fee
        - entry_position.average_entry_price * protective_fill.quantity;
    report.record_trade(realized, protective_fill.fee);
    report.record_slippage(protective_fill.slippage);
    report.record_protective_exit();
    store.save_fill(
        run_id,
        &fill_record(&protective_fill, DEMO_PROTECTIVE_FILL_UUID),
    )?;
    store.append_event(AuditEvent::new(
        run_id,
        "protective_exit",
        plan.execution_id.to_string(),
        protective_fill.timestamp,
        serde_json::to_value(&protective_fill)?,
    ))?;

    let portfolio = executor.engine().portfolio_snapshot()?;
    report.record_equity(portfolio.equity);
    let report = report.finish(portfolio.equity);
    store.save_portfolio_snapshot(
        run_id,
        Uuid::from_u128(0x50524f4f465452414445534e41503031),
        protective_fill.timestamp,
        serde_json::to_value(&portfolio)?,
    )?;
    drop(store);

    let projector = OperatorProjector::new(
        AuditStore::open(database_path)?,
        OperatorReadModel::default(),
    );
    projector
        .run_started(
            run_id,
            DEMO_SCENARIO,
            OperatingMode::Simulation,
            SafetyState::Normal,
            entry_time,
        )
        .map_err(|error| DemoError::Operator(error.to_string()))?;
    projector
        .market_updated(run_id, quote, entry_time)
        .map_err(|error| DemoError::Operator(error.to_string()))?;
    projector
        .execution_planned(run_id, &plan, entry_time)
        .map_err(|error| DemoError::Operator(error.to_string()))?;
    projector
        .execution_updated(run_id, &plan, &entry_receipt, entry_time)
        .map_err(|error| DemoError::Operator(error.to_string()))?;
    projector
        .portfolio_updated(
            run_id,
            PortfolioView {
                cash: portfolio.cash.to_string(),
                equity: portfolio.equity.to_string(),
                realized_pnl: portfolio.realized_pnl.to_string(),
                fees: portfolio.fees_paid.to_string(),
                available_cash: Some(portfolio.cash.to_string()),
                unrealized_pnl: Some(Decimal::ZERO.to_string()),
                pending_orders: executor.engine().pending_orders().len(),
                marks: std::collections::BTreeMap::from([(
                    instrument.to_string(),
                    "flat".to_owned(),
                )]),
                observed_at: Some(protective_fill.timestamp),
            },
            Vec::new(),
            protective_fill.timestamp,
        )
        .map_err(|error| DemoError::Operator(error.to_string()))?;
    projector
        .run_completed(run_id, protective_fill.timestamp)
        .map_err(|error| DemoError::Operator(error.to_string()))?;
    let operator_revision = projector.snapshot().revision;
    drop(projector);

    let store = AuditStore::open(database_path)?;
    let replay_event_count = store.replay(decision.decision_id.as_uuid())?.events.len();
    let audit_event_count = store.count_events_for_run(run_id)?;

    let receipt = DemoReceipt {
        schema_version: 1,
        scenario: DEMO_SCENARIO.to_owned(),
        fixture_label: DEMO_FIXTURE_LABEL.to_owned(),
        environment: "SIMULATION".to_owned(),
        run_id: run_id.as_uuid().to_string(),
        decision_id: decision.decision_id.as_uuid().to_string(),
        risk_decision_id: risk.risk_decision_id.to_string(),
        execution_id: plan.execution_id.to_string(),
        audit_event_count,
        replay_event_count,
        operator_revision,
        report: serde_json::to_value(report)?,
    };
    Ok(receipt)
}

fn receipt_from_fill(plan: &ExecutionPlan, fill: &SimulationFill) -> ExecutionReceipt {
    ExecutionReceipt {
        execution_id: plan.execution_id,
        client_order_id: plan.client_order_id.clone(),
        exchange_order_id: None,
        state: ExecutionState::Filled,
        filled_quantity: fill.quantity,
        remaining_quantity: Some(Decimal::ZERO),
        average_price: Some(fill.price),
    }
}

fn fill_record(fill: &SimulationFill, fill_id: Uuid) -> FillRecord {
    FillRecord {
        execution_id: DEMO_EXECUTION_UUID,
        fill_id,
        occurred_at: fill.timestamp,
        quantity: fill.quantity,
        price: fill.price,
        fee: fill.fee,
        payload: serde_json::to_value(fill).unwrap_or_else(|_| json!({"serialization": "failed"})),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simulation_demo_is_a_replayable_synthetic_run() {
        let receipt = run_simulation_demo(None, None).expect("demo should run");

        assert_eq!(receipt.schema_version, 1);
        assert_eq!(receipt.environment, "SIMULATION");
        assert_eq!(receipt.fixture_label, "deterministic-fixture");
        assert!(receipt.audit_event_count >= 5);
        assert!(receipt.replay_event_count >= 2);
        assert!(receipt.operator_revision >= 1);
        assert_eq!(receipt.report["starting_equity"], "30");
    }

    #[test]
    fn simulation_demo_can_write_a_json_receipt_and_sqlite_audit() {
        let directory =
            std::env::temp_dir().join(format!("prooftrade-demo-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&directory).expect("temp directory");
        let database = directory.join("run.sqlite");
        let receipt_path = directory.join("receipt.json");

        let receipt = run_simulation_demo(Some(&database), Some(&receipt_path)).unwrap();
        let saved: DemoReceipt =
            serde_json::from_str(&std::fs::read_to_string(&receipt_path).expect("receipt file"))
                .unwrap();

        assert_eq!(saved, receipt);
        assert!(database.exists());
        std::fs::remove_dir_all(directory).expect("cleanup");
    }
}
