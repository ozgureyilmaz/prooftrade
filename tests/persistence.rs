use prooftrade::domain::{DecisionId, Instrument, TradeDecision};
use prooftrade::execution::{ExecutionPolicy, ExecutionReceipt, ExecutionState, MarketQuote};
use prooftrade::persistence::{AuditEvent, AuditStore, FillRecord, RunId};
use prooftrade::risk::{
    AccountState, OperatingMode, RiskConfig, RiskEngine, RiskSnapshot, SafetyState,
};
use rust_decimal::Decimal;
use serde_json::json;
use std::path::PathBuf;
use time::OffsetDateTime;
use uuid::Uuid;

fn now() -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap()
}

fn path() -> PathBuf {
    std::env::temp_dir().join(format!("prooftrade-test-{}.sqlite", Uuid::new_v4()))
}

fn decision() -> TradeDecision {
    TradeDecision::buy(
        DecisionId::from_uuid(Uuid::from_u128(501)),
        Instrument::parse("BTC-USDT").unwrap(),
        now(),
        Decimal::from(100),
        Decimal::new(10, 2),
    )
}

#[test]
fn migration_and_wal_configuration_are_applied() {
    let path = path();
    let store = AuditStore::open(&path).unwrap();
    assert_eq!(store.migration_version().unwrap(), 1);
    assert_eq!(store.journal_mode().unwrap().to_ascii_lowercase(), "wal");
    drop(store);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn operator_projection_is_persisted_and_reloaded_by_run() {
    let path = path();
    let run_id = RunId::new();
    {
        let mut store = AuditStore::open(&path).unwrap();
        store
            .append_event_with_projection(
                AuditEvent::new(
                    run_id,
                    "run_started",
                    run_id.as_uuid().to_string(),
                    now(),
                    json!({"phase":"researching"}),
                ),
                7,
                json!({"revision":7,"run_id":run_id.as_uuid().to_string()}),
            )
            .unwrap();

        let current = store.load_current_operator_projection().unwrap().unwrap();
        assert_eq!(current.run_id, run_id);
        assert_eq!(current.revision, 7);
        assert_eq!(current.payload["revision"], 7);
        assert_eq!(store.load_latest_run().unwrap(), Some(run_id));
    }

    let store = AuditStore::open(&path).unwrap();
    let by_run = store.load_operator_projection(run_id).unwrap().unwrap();
    assert_eq!(by_run.revision, 7);
    assert_eq!(by_run.payload["run_id"], run_id.as_uuid().to_string());
    drop(store);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn decision_risk_and_replay_events_are_linked() {
    let path = path();
    let mut store = AuditStore::open(&path).unwrap();
    let decision = decision();
    let risk = RiskEngine::new(RiskConfig::default()).evaluate(
        &decision,
        &RiskSnapshot::new(
            now(),
            AccountState::new(Decimal::from(200), Decimal::from(200), Decimal::from(200)).unwrap(),
            vec![],
            OperatingMode::Simulation,
            SafetyState::Normal,
        )
        .unwrap(),
    );
    let run_id = RunId::new();
    store
        .save_decision_bundle(run_id, &decision, &risk)
        .unwrap();
    let quote = MarketQuote::new(
        Instrument::parse("BTC-USDT").unwrap(),
        now(),
        Decimal::from(99),
        Decimal::from(100),
    )
    .unwrap();
    let plan = ExecutionPolicy::default()
        .plan(&risk, &decision, &quote)
        .unwrap();
    let receipt = ExecutionReceipt {
        execution_id: plan.execution_id,
        client_order_id: plan.client_order_id.clone(),
        exchange_order_id: Some("exchange-1".to_owned()),
        state: ExecutionState::Filled,
        filled_quantity: Decimal::ONE,
        remaining_quantity: Some(Decimal::ZERO),
        average_price: Some(Decimal::from(100)),
    };
    store.save_execution(run_id, &plan, &receipt).unwrap();
    store
        .save_fill(
            run_id,
            &FillRecord {
                execution_id: plan.execution_id,
                fill_id: Uuid::from_u128(503),
                occurred_at: now(),
                quantity: Decimal::ONE,
                price: Decimal::from(100),
                fee: Decimal::new(25, 2),
                payload: json!({"fee":"0.25"}),
            },
        )
        .unwrap();
    let replay = store.replay(decision.decision_id.as_uuid()).unwrap();
    assert!(
        replay
            .events
            .iter()
            .any(|event| event.event_type == "decision")
    );
    assert!(
        replay
            .events
            .iter()
            .any(|event| event.event_type == "risk_decision")
    );
    assert!(
        replay
            .events
            .iter()
            .any(|event| event.event_type == "execution")
    );
    assert!(!replay.serialized().contains("api_secret"));
    drop(store);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn duplicate_decision_rolls_back_the_whole_bundle() {
    let path = path();
    let mut store = AuditStore::open(&path).unwrap();
    let decision = decision();
    let risk = RiskEngine::new(RiskConfig::default()).evaluate(
        &decision,
        &RiskSnapshot::new(
            now(),
            AccountState::new(Decimal::from(200), Decimal::from(200), Decimal::from(200)).unwrap(),
            vec![],
            OperatingMode::Simulation,
            SafetyState::Normal,
        )
        .unwrap(),
    );
    let run_id = RunId::new();
    store
        .save_decision_bundle(run_id, &decision, &risk)
        .unwrap();
    assert!(
        store
            .save_decision_bundle(run_id, &decision, &risk)
            .is_err()
    );
    assert_eq!(
        store
            .count_events_for(decision.decision_id.as_uuid())
            .unwrap(),
        2
    );
    drop(store);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn append_event_redacts_secret_fields_and_reports_unknown_executions() {
    let path = path();
    let mut store = AuditStore::open(&path).unwrap();
    let decision_id = Uuid::from_u128(502);
    store
        .append_event(AuditEvent::new(
            RunId::new(),
            "execution",
            decision_id.to_string(),
            now(),
            json!({"status":"Unknown","execution_id":"00000000-0000-0000-0000-0000000001f8","apiKey":"do-not-store","nested":{"passphrase":"nope","private-key":"nope2"}}),
        ))
        .unwrap();
    let report = store.report().unwrap();
    assert_eq!(report.unknown_executions, 1);
    assert_eq!(
        store.unresolved_execution_ids().unwrap(),
        vec![Uuid::from_u128(504)]
    );
    store
        .append_event(AuditEvent::new(
            RunId::new(),
            "fill",
            decision_id.to_string(),
            now(),
            json!({"fee":"2.50"}),
        ))
        .unwrap();
    assert_eq!(store.report().unwrap().fees, Decimal::new(250, 2));
    let replay = store.replay(decision_id).unwrap();
    let serialized = replay.serialized();
    assert!(!serialized.contains("do-not-store"));
    assert!(!serialized.contains("passphrase"));
    assert!(!serialized.contains("nope2"));
    drop(store);
    std::fs::remove_file(path).unwrap();
}
