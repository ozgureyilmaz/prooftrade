use prooftrade::domain::{DecisionId, Instrument, TradeDecision};
use prooftrade::execution::{
    ExecutionError, ExecutionGateway, ExecutionPlan, ExecutionState, GatewayOrderStatus,
    GatewayPlacement, MarketQuote,
};
use prooftrade::operator::{OperatorController, OperatorProjector, OperatorReadModel};
use prooftrade::persistence::{AuditStore, RunId};
use prooftrade::risk::{AccountState, OperatingMode, RiskConfig, RiskSnapshot, SafetyState};
use prooftrade::runtime::{RuntimeCoordinator, RuntimeOutcome};
use rust_decimal::Decimal;
use time::OffsetDateTime;
use uuid::Uuid;

fn now() -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap()
}

fn decision() -> TradeDecision {
    TradeDecision::buy(
        DecisionId::from_uuid(Uuid::from_u128(701)),
        Instrument::parse("BTC-USDT").unwrap(),
        now(),
        Decimal::from(100),
        Decimal::new(10, 2),
    )
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
        vec![],
        OperatingMode::Simulation,
        SafetyState::Normal,
    )
    .unwrap()
}

fn quote() -> MarketQuote {
    MarketQuote::new(
        Instrument::parse("BTC-USDT").unwrap(),
        now(),
        Decimal::from(99),
        Decimal::from(100),
    )
    .unwrap()
}

#[derive(Default)]
struct FakeGateway {
    calls: usize,
}

impl ExecutionGateway for FakeGateway {
    fn place(&mut self, _plan: &ExecutionPlan) -> GatewayPlacement {
        self.calls += 1;
        GatewayPlacement::Acknowledged {
            exchange_order_id: "sim-order".to_owned(),
        }
    }

    fn query(
        &mut self,
        _instrument: &Instrument,
        _client_order_id: &str,
    ) -> Result<GatewayOrderStatus, ExecutionError> {
        Err(ExecutionError::Gateway("not needed".to_owned()))
    }

    fn cancel(&mut self, _instrument: &Instrument, _client_order_id: &str) -> GatewayPlacement {
        GatewayPlacement::Rejected {
            code: "not-needed".to_owned(),
            message: "not needed".to_owned(),
        }
    }

    fn is_available(&self) -> bool {
        true
    }
}

#[test]
fn runtime_executes_only_a_risk_approved_intent() {
    let mut runtime = RuntimeCoordinator::new(
        RiskConfig::default(),
        Default::default(),
        FakeGateway::default(),
    );
    let outcome = runtime.process(decision(), snapshot(200), quote()).unwrap();
    assert!(
        matches!(outcome, RuntimeOutcome::Executed { receipt, .. } if receipt.state == ExecutionState::Acknowledged)
    );
    assert_eq!(runtime.execution().gateway().calls, 1);
}

#[test]
fn runtime_rejects_unaffordable_intent_without_gateway_call() {
    let mut runtime = RuntimeCoordinator::new(
        RiskConfig::default(),
        Default::default(),
        FakeGateway::default(),
    );
    let outcome = runtime.process(decision(), snapshot(10), quote()).unwrap();
    assert!(matches!(outcome, RuntimeOutcome::Rejected { .. }));
    assert_eq!(runtime.execution().gateway().calls, 0);
}

#[test]
fn operator_kill_blocks_buy_in_the_runtime() {
    let controller = OperatorController::new(SafetyState::Normal);
    let mut runtime = RuntimeCoordinator::with_safety_state_store(
        RiskConfig::default(),
        Default::default(),
        FakeGateway::default(),
        controller.safety_state_store(),
    );
    controller.kill_switch("KILL").unwrap();

    let outcome = runtime.process(decision(), snapshot(200), quote()).unwrap();

    assert!(matches!(outcome, RuntimeOutcome::Rejected { .. }));
    assert_eq!(runtime.execution().gateway().calls, 0);
}

#[test]
fn runtime_process_projects_risk_and_execution_lifecycle() {
    let projector = OperatorProjector::new(
        AuditStore::in_memory().unwrap(),
        OperatorReadModel::default(),
    );
    let mut runtime = RuntimeCoordinator::new(
        RiskConfig::default(),
        Default::default(),
        FakeGateway::default(),
    );

    let outcome = runtime
        .process_with_projection(RunId::new(), decision(), snapshot(200), quote(), &projector)
        .unwrap();

    assert!(matches!(outcome, RuntimeOutcome::Executed { .. }));
    let model = projector.snapshot();
    assert!(model.latest_risk_decision.is_some());
    assert_eq!(model.execution_records.len(), 1);
    assert_eq!(model.executions.len(), 1);
    assert_eq!(model.revision, 3);
}
