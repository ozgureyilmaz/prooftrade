use prooftrade::domain::{
    DecisionId, Instrument, PositionId, PositionState, TradeAction, TradeDecision,
};
use prooftrade::risk::{
    AccountState, OperatingMode, RiskConfig, RiskEngine, RiskOutcome, RiskReasonCode, RiskSnapshot,
    SafetyState, UnresolvedExecution,
};
use rust_decimal::Decimal;
use time::OffsetDateTime;
use uuid::Uuid;

fn now() -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap()
}

fn instrument(symbol: &str) -> Instrument {
    Instrument::parse(symbol).unwrap()
}

fn buy(id: u128, symbol: &str, notional: i64) -> TradeDecision {
    TradeDecision::buy(
        DecisionId::from_uuid(Uuid::from_u128(id)),
        instrument(symbol),
        now(),
        Decimal::from(notional),
        Decimal::new(10, 2),
    )
}

fn open(id: u128, symbol: &str) -> PositionState {
    PositionState::open(
        PositionId::from_uuid(Uuid::from_u128(id)),
        instrument(symbol),
        Decimal::ONE,
        Decimal::from(100),
        now(),
        now(),
    )
    .unwrap()
}

fn snapshot(cash: i64) -> RiskSnapshot {
    RiskSnapshot::new(
        now(),
        AccountState::new(
            Decimal::from(cash),
            Decimal::from(cash),
            Decimal::from(cash),
        )
        .unwrap(),
        Vec::new(),
        OperatingMode::Simulation,
        SafetyState::Normal,
    )
    .unwrap()
}

#[test]
fn valid_buy_is_approved_without_resizing_in_simulation() {
    let result =
        RiskEngine::new(RiskConfig::default()).evaluate(&buy(1, "BTC-USDT", 100), &snapshot(200));

    assert_eq!(result.outcome, RiskOutcome::Approved);
    assert_eq!(
        result.approved_intent.unwrap().notional,
        Some(Decimal::from(100))
    );
}

#[test]
fn sell_without_inventory_is_rejected_with_a_typed_reason() {
    let decision = TradeDecision::sell(
        DecisionId::from_uuid(Uuid::from_u128(2)),
        instrument("BTC-USDT"),
        now(),
        Decimal::new(50, 2),
    );
    let result = RiskEngine::new(RiskConfig::default()).evaluate(&decision, &snapshot(200));

    assert_eq!(result.outcome, RiskOutcome::Rejected);
    assert!(
        result
            .reasons
            .contains(&RiskReasonCode::InsufficientInventory)
    );
}

#[test]
fn fourth_open_position_is_rejected_but_adds_are_not_new_positions() {
    let mut state = snapshot(1_000).with_positions(vec![
        open(10, "BTC-USDT"),
        open(11, "ETH-USDT"),
        open(12, "SOL-USDT"),
    ]);
    let fourth = buy(3, "HYPE-USDT", 100);
    let result = RiskEngine::new(RiskConfig::default()).evaluate(&fourth, &state);
    assert!(
        result
            .reasons
            .contains(&RiskReasonCode::PositionLimitReached)
    );

    state = state
        .with_positions(vec![
            open(10, "BTC-USDT"),
            open(11, "ETH-USDT"),
            open(12, "SOL-USDT"),
        ])
        .with_add_evidence_qualified(true);
    let add = buy(4, "BTC-USDT", 100);
    let result = RiskEngine::new(RiskConfig::default()).evaluate(&add, &state);
    assert_eq!(result.outcome, RiskOutcome::Approved);
}

#[test]
fn stale_decision_is_rejected() {
    let mut decision = buy(5, "BTC-USDT", 100);
    decision.created_at = now() - time::Duration::seconds(61);
    let config = RiskConfig::default().with_max_decision_age_secs(60);
    let result = RiskEngine::new(config).evaluate(&decision, &snapshot(200));
    assert!(result.reasons.contains(&RiskReasonCode::DecisionStale));
}

#[test]
fn unresolved_related_execution_blocks_new_risk() {
    let state = snapshot(200).with_unresolved_execution(UnresolvedExecution::new(
        instrument("BTC-USDT"),
        TradeAction::Buy,
        "client-1",
    ));
    let result = RiskEngine::new(RiskConfig::default()).evaluate(&buy(6, "BTC-USDT", 100), &state);
    assert!(
        result
            .reasons
            .contains(&RiskReasonCode::ExecutionStateUnknown)
    );
}

#[test]
fn kill_switch_blocks_buy_and_safe_state_allows_protective_sell() {
    let killed = snapshot(200).with_safety_state(SafetyState::Killed);
    let result = RiskEngine::new(RiskConfig::default()).evaluate(&buy(7, "BTC-USDT", 100), &killed);
    assert!(result.reasons.contains(&RiskReasonCode::TradingKilled));

    let protective = TradeDecision::sell(
        DecisionId::from_uuid(Uuid::from_u128(8)),
        instrument("BTC-USDT"),
        now(),
        Decimal::ONE,
    );
    let state = snapshot(0)
        .with_positions(vec![open(20, "BTC-USDT")])
        .with_safety_state(SafetyState::Safe)
        .with_protective_exit(true);
    let result = RiskEngine::new(RiskConfig::default()).evaluate(&protective, &state);
    assert_eq!(result.outcome, RiskOutcome::Approved);
}

#[test]
fn live_policy_can_return_constrained_approval() {
    let config = RiskConfig::default().with_max_trade_notional(Decimal::from(50));
    let state = snapshot(200).with_mode(OperatingMode::Live);
    let result = RiskEngine::new(config).evaluate(&buy(9, "BTC-USDT", 100), &state);
    assert_eq!(result.outcome, RiskOutcome::ApprovedWithConstraints);
    assert_eq!(
        result.approved_intent.unwrap().notional,
        Some(Decimal::from(50))
    );
}

#[test]
fn add_without_new_evidence_is_rejected() {
    let state = snapshot(200)
        .with_positions(vec![open(30, "BTC-USDT")])
        .with_add_evidence_qualified(false);
    let result = RiskEngine::new(RiskConfig::default()).evaluate(&buy(10, "BTC-USDT", 100), &state);
    assert_eq!(result.outcome, RiskOutcome::Rejected);
    assert!(
        result
            .reasons
            .contains(&RiskReasonCode::AddWithoutNewEvidence)
    );
}

#[test]
fn configured_session_loss_guardrail_blocks_new_buy() {
    let config = RiskConfig::default().with_max_session_loss(Decimal::from(10));
    let state = snapshot(200).with_session_loss(Decimal::from(11));
    let result = RiskEngine::new(config).evaluate(&buy(11, "BTC-USDT", 100), &state);
    assert_eq!(result.outcome, RiskOutcome::Rejected);
    assert!(
        result
            .reasons
            .contains(&RiskReasonCode::SessionLossExceeded)
    );
}
