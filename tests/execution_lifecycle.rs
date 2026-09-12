use std::collections::VecDeque;

use prooftrade::domain::{DecisionId, Instrument, TradeDecision, Urgency};
use prooftrade::execution::{
    ExecutionCheckpoint, ExecutionEngine, ExecutionError, ExecutionGateway, ExecutionPolicy,
    ExecutionState, GatewayOrderState, GatewayOrderStatus, GatewayPlacement, MarketQuote,
    PlannedOrderType,
};
use prooftrade::risk::{
    AccountState, OperatingMode, RiskConfig, RiskEngine, RiskSnapshot, SafetyState,
};
use rust_decimal::Decimal;
use time::OffsetDateTime;
use uuid::Uuid;

fn now() -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap()
}

fn instrument() -> Instrument {
    Instrument::parse("BTC-USDT").unwrap()
}

fn decision() -> TradeDecision {
    let mut decision = TradeDecision::buy(
        DecisionId::from_uuid(Uuid::from_u128(101)),
        instrument(),
        now(),
        Decimal::from(100),
        Decimal::new(10, 2),
    );
    decision.confidence = Decimal::new(80, 2);
    decision.urgency = Urgency::High;
    decision
}

fn approved() -> prooftrade::risk::RiskDecision {
    RiskEngine::new(RiskConfig::default()).evaluate(
        &decision(),
        &RiskSnapshot::new(
            now(),
            AccountState::new(Decimal::from(200), Decimal::from(200), Decimal::from(200)).unwrap(),
            vec![],
            OperatingMode::Simulation,
            SafetyState::Normal,
        )
        .unwrap(),
    )
}

fn quote() -> MarketQuote {
    MarketQuote::new(instrument(), now(), Decimal::from(99), Decimal::from(100)).unwrap()
}

#[derive(Default)]
struct FakeGateway {
    placements: usize,
    placement: Option<GatewayPlacement>,
    queries: VecDeque<GatewayOrderStatus>,
    available: bool,
}

impl ExecutionGateway for FakeGateway {
    fn place(&mut self, plan: &prooftrade::execution::ExecutionPlan) -> GatewayPlacement {
        self.placements += 1;
        self.placement
            .clone()
            .unwrap_or(GatewayPlacement::Acknowledged {
                exchange_order_id: format!("exchange-{}", plan.client_order_id),
            })
    }

    fn query(
        &mut self,
        _instrument: &Instrument,
        _client_order_id: &str,
    ) -> Result<GatewayOrderStatus, ExecutionError> {
        self.queries
            .pop_front()
            .ok_or_else(|| ExecutionError::Gateway("no scripted query".to_owned()))
    }

    fn cancel(&mut self, _instrument: &Instrument, _client_order_id: &str) -> GatewayPlacement {
        GatewayPlacement::Acknowledged {
            exchange_order_id: "cancelled".to_owned(),
        }
    }

    fn is_available(&self) -> bool {
        self.available
    }
}

struct LiveGateway;

impl ExecutionGateway for LiveGateway {
    fn place(&mut self, _plan: &prooftrade::execution::ExecutionPlan) -> GatewayPlacement {
        panic!("live gateway must not be reached in this test")
    }

    fn query(
        &mut self,
        _instrument: &Instrument,
        _client_order_id: &str,
    ) -> Result<GatewayOrderStatus, ExecutionError> {
        Err(ExecutionError::Gateway("not expected".to_owned()))
    }

    fn cancel(&mut self, _instrument: &Instrument, _client_order_id: &str) -> GatewayPlacement {
        GatewayPlacement::Unknown {
            reason: "not expected".to_owned(),
        }
    }

    fn is_available(&self) -> bool {
        true
    }

    fn operating_mode(&self) -> OperatingMode {
        OperatingMode::Live
    }
}

#[test]
fn partial_fill_remains_explicit_for_re_evaluation() {
    let gateway = FakeGateway {
        available: true,
        placement: Some(GatewayPlacement::Unknown {
            reason: "timeout".to_owned(),
        }),
        queries: VecDeque::from([GatewayOrderStatus {
            state: GatewayOrderState::PartiallyFilled,
            filled_quantity: Decimal::new(4, 1),
            remaining_quantity: Some(Decimal::new(6, 1)),
            average_price: Some(Decimal::from(100)),
        }]),
        ..Default::default()
    };
    let mut engine = ExecutionEngine::new(gateway);
    let risk = approved();
    let d = decision();
    let plan = ExecutionPolicy::default()
        .plan(&risk, &d, &quote())
        .unwrap();
    let first = engine.execute(&risk, plan).unwrap();
    let partial = engine.reconcile(first.execution_id).unwrap();
    assert_eq!(partial.state, ExecutionState::PartiallyFilled);
    assert_eq!(partial.remaining_quantity, Some(Decimal::new(6, 1)));
}

#[test]
fn restart_requires_reconciliation_before_new_risk() {
    let gateway = FakeGateway {
        available: true,
        placement: Some(GatewayPlacement::Unknown {
            reason: "timeout".to_owned(),
        }),
        ..Default::default()
    };
    let mut engine = ExecutionEngine::new(gateway);
    let risk = approved();
    let d = decision();
    let plan = ExecutionPolicy::default()
        .plan(&risk, &d, &quote())
        .unwrap();
    let receipt = engine.execute(&risk, plan).unwrap();
    assert!(!engine.can_accept_new_risk());

    let checkpoints = engine.checkpoints();
    assert_eq!(checkpoints.len(), 1);
    let gateway = FakeGateway {
        available: true,
        queries: VecDeque::from([GatewayOrderStatus::filled(Decimal::ONE, Decimal::from(100))]),
        ..Default::default()
    };
    let mut restarted = ExecutionEngine::restore(
        gateway,
        vec![ExecutionCheckpoint {
            plan: engine.checkpoints()[0].plan.clone(),
            receipt,
        }],
    );
    assert!(!restarted.can_accept_new_risk());
    assert!(restarted.startup_reconcile().unwrap());
    assert!(restarted.can_accept_new_risk());
}

#[test]
fn restart_reconciles_acknowledged_order_state() {
    let gateway = FakeGateway {
        available: true,
        queries: VecDeque::from([GatewayOrderStatus::filled(Decimal::ONE, Decimal::from(100))]),
        ..Default::default()
    };
    let risk = approved();
    let d = decision();
    let plan = ExecutionPolicy::default()
        .plan(&risk, &d, &quote())
        .unwrap();
    let receipt = prooftrade::execution::ExecutionReceipt {
        execution_id: plan.execution_id,
        client_order_id: plan.client_order_id.clone(),
        exchange_order_id: Some("exchange-ack".to_owned()),
        state: ExecutionState::Acknowledged,
        filled_quantity: Decimal::ZERO,
        remaining_quantity: Some(Decimal::ONE),
        average_price: None,
    };
    let mut restarted =
        ExecutionEngine::restore(gateway, vec![ExecutionCheckpoint { plan, receipt }]);

    assert!(!restarted.can_accept_new_risk());
    assert!(restarted.startup_reconcile().unwrap());
    assert!(restarted.can_accept_new_risk());
}

#[test]
fn approved_buy_creates_market_plan_for_high_urgency() {
    let plan = ExecutionPolicy::default()
        .plan(&approved(), &decision(), &quote())
        .unwrap();
    assert_eq!(plan.order_type, PlannedOrderType::Market);
}

#[test]
fn lower_urgency_creates_bounded_limit_plan() {
    let mut d = decision();
    d.urgency = Urgency::Low;
    d.max_acceptable_price = Some(Decimal::from(101));
    let plan = ExecutionPolicy::default()
        .plan(&approved_for(&d), &d, &quote())
        .unwrap();
    assert_eq!(
        plan.order_type,
        PlannedOrderType::Limit {
            price: Decimal::from(101)
        }
    );
}

#[test]
fn unsafe_market_price_boundary_prevents_market_plan() {
    let mut d = decision();
    d.max_acceptable_price = Some(Decimal::from(99));
    let error = ExecutionPolicy::default()
        .plan(&approved_for(&d), &d, &quote())
        .unwrap_err();
    assert!(matches!(error, ExecutionError::PriceConstraint));
}

#[test]
fn wide_spread_prevents_urgent_market_plan() {
    let policy = ExecutionPolicy::default();
    let wide_quote =
        MarketQuote::new(instrument(), now(), Decimal::from(90), Decimal::from(100)).unwrap();
    let error = policy
        .plan(&approved(), &decision(), &wide_quote)
        .unwrap_err();
    assert!(matches!(error, ExecutionError::PriceConstraint));
}

#[test]
fn stale_limit_is_cancelled_and_confirmed_by_reconciliation() {
    let mut d = decision();
    d.urgency = Urgency::Low;
    d.max_acceptable_price = Some(Decimal::from(101));
    let risk = approved_for(&d);
    let plan = ExecutionPolicy::default()
        .plan(&risk, &d, &quote())
        .unwrap();
    let gateway = FakeGateway {
        available: true,
        queries: VecDeque::from([GatewayOrderStatus {
            state: GatewayOrderState::Cancelled,
            filled_quantity: Decimal::ZERO,
            remaining_quantity: Some(Decimal::from(1)),
            average_price: None,
        }]),
        ..Default::default()
    };
    let mut engine = ExecutionEngine::new(gateway);
    let submitted = engine.execute(&risk, plan).unwrap();
    let expired = engine
        .expire_limit(
            submitted.execution_id,
            now() + time::Duration::seconds(61),
            &ExecutionPolicy::default(),
        )
        .unwrap();
    assert_eq!(expired.state, ExecutionState::Cancelled);
}

#[test]
fn rejected_risk_decision_cannot_execute() {
    let gateway = FakeGateway {
        available: true,
        ..Default::default()
    };
    let mut engine = ExecutionEngine::new(gateway);
    let rejected = RiskEngine::new(RiskConfig::default()).evaluate(
        &decision(),
        &RiskSnapshot::new(
            now(),
            AccountState::new(Decimal::from(10), Decimal::from(10), Decimal::from(10)).unwrap(),
            vec![],
            OperatingMode::Simulation,
            SafetyState::Normal,
        )
        .unwrap(),
    );
    let valid_risk = approved();
    let valid_decision = decision();
    let plan = ExecutionPolicy::default()
        .plan(&valid_risk, &valid_decision, &quote())
        .unwrap();
    let error = engine.execute(&rejected, plan).unwrap_err();
    assert!(matches!(error, ExecutionError::RiskNotApproved));
}

#[test]
fn simulation_plan_cannot_reach_live_gateway() {
    let risk = approved();
    let d = decision();
    let plan = ExecutionPolicy::default()
        .plan(&risk, &d, &quote())
        .unwrap();
    let mut engine = ExecutionEngine::new(LiveGateway);

    let error = engine.execute(&risk, plan).unwrap_err();

    assert!(matches!(error, ExecutionError::EnvironmentMismatch));
}

#[test]
fn execution_plan_action_and_size_must_match_approved_intent() {
    let risk = approved();
    let d = decision();
    let mut plan = ExecutionPolicy::default()
        .plan(&risk, &d, &quote())
        .unwrap();
    plan.action = prooftrade::domain::TradeAction::Sell;
    let mut engine = ExecutionEngine::new(FakeGateway {
        available: true,
        ..Default::default()
    });

    let error = engine.execute(&risk, plan).unwrap_err();

    assert!(matches!(error, ExecutionError::RiskPlanMismatch));
    assert_eq!(engine.gateway().placements, 0);
}

#[test]
fn placement_timeout_is_unknown_and_never_blindly_retried() {
    let gateway = FakeGateway {
        available: true,
        placement: Some(GatewayPlacement::Unknown {
            reason: "timeout".to_owned(),
        }),
        ..Default::default()
    };
    let mut engine = ExecutionEngine::new(gateway);
    let valid_risk = approved();
    let valid_decision = decision();
    let plan = ExecutionPolicy::default()
        .plan(&valid_risk, &valid_decision, &quote())
        .unwrap();
    let first = engine.execute(&valid_risk, plan.clone()).unwrap();
    assert_eq!(first.state, ExecutionState::Unknown);
    let error = engine.execute(&valid_risk, plan).unwrap_err();
    assert!(matches!(error, ExecutionError::ReconciliationRequired));
    assert_eq!(engine.gateway().placements, 1);
}

#[test]
fn unknown_order_can_be_resolved_by_query_without_new_placement() {
    let gateway = FakeGateway {
        available: true,
        placement: Some(GatewayPlacement::Unknown {
            reason: "timeout".to_owned(),
        }),
        queries: VecDeque::from([GatewayOrderStatus::filled(
            Decimal::from(1),
            Decimal::from(100),
        )]),
        ..Default::default()
    };
    let mut engine = ExecutionEngine::new(gateway);
    let valid_risk = approved();
    let valid_decision = decision();
    let plan = ExecutionPolicy::default()
        .plan(&valid_risk, &valid_decision, &quote())
        .unwrap();
    let first = engine.execute(&valid_risk, plan).unwrap();
    let resolved = engine.reconcile(first.execution_id).unwrap();
    assert_eq!(resolved.state, ExecutionState::Filled);
    assert_eq!(engine.gateway().placements, 1);
}

#[test]
fn unavailable_gateway_blocks_new_mutation() {
    let gateway = FakeGateway {
        available: false,
        ..Default::default()
    };
    let mut engine = ExecutionEngine::new(gateway);
    let valid_risk = approved();
    let valid_decision = decision();
    let plan = ExecutionPolicy::default()
        .plan(&valid_risk, &valid_decision, &quote())
        .unwrap();
    let error = engine.execute(&valid_risk, plan).unwrap_err();
    assert!(matches!(error, ExecutionError::GatewayUnavailable));
}

fn approved_for(decision: &TradeDecision) -> prooftrade::risk::RiskDecision {
    RiskEngine::new(RiskConfig::default()).evaluate(
        decision,
        &RiskSnapshot::new(
            now(),
            AccountState::new(Decimal::from(200), Decimal::from(200), Decimal::from(200)).unwrap(),
            vec![],
            OperatingMode::Simulation,
            SafetyState::Normal,
        )
        .unwrap(),
    )
}
