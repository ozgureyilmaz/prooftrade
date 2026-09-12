use prooftrade::domain::{DecisionId, Instrument, TradeDecision};
use prooftrade::execution::{ExecutionPolicy, MarketQuote, SimulationExecutor};
use prooftrade::risk::{
    AccountState, OperatingMode, RiskConfig, RiskEngine, RiskSnapshot, SafetyState,
};
use prooftrade::simulation::{
    ExecutionReason, MarketSnapshot, OrderStatus, SimulationConfig, SimulationEngine,
};
use rust_decimal::Decimal;
use time::OffsetDateTime;
use uuid::Uuid;

fn now() -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap()
}

fn decision() -> TradeDecision {
    TradeDecision::buy(
        DecisionId::from_uuid(Uuid::from_u128(901)),
        Instrument::parse("BTC-USDT").unwrap(),
        now(),
        Decimal::from(100),
        Decimal::new(10, 2),
    )
}

#[test]
fn deterministic_end_to_end_simulation_runs_entry_and_hard_stop() {
    let instrument = Instrument::parse("BTC-USDT").unwrap();
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
    let quote = MarketQuote::new(
        instrument.clone(),
        now(),
        Decimal::from(99),
        Decimal::from(100),
    )
    .unwrap();
    let plan = ExecutionPolicy::default()
        .plan(&risk, &decision, &quote)
        .unwrap();
    let entry_snapshot = MarketSnapshot::new(
        instrument.clone(),
        now(),
        Decimal::from(99),
        Decimal::from(100),
    )
    .unwrap();
    let mut simulation = SimulationExecutor::new(
        SimulationEngine::new(SimulationConfig::new(Decimal::from(200))).unwrap(),
    );
    let entry = simulation.execute(&plan, entry_snapshot).unwrap();
    assert_eq!(entry.status, OrderStatus::Filled);
    assert!(simulation.engine().position(&instrument).is_some());

    let stop_snapshot = MarketSnapshot::new(
        instrument,
        now() + time::Duration::seconds(1),
        Decimal::from(85),
        Decimal::from(86),
    )
    .unwrap();
    let exits = simulation
        .engine_mut()
        .on_market_snapshot(stop_snapshot)
        .unwrap();
    assert_eq!(exits.len(), 1);
    assert_eq!(
        exits[0].fill.as_ref().unwrap().reason,
        ExecutionReason::HardStopLoss
    );
    assert!(simulation.engine().positions().is_empty());
}
