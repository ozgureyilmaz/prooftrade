//! Small in-process composition of decision intent, risk, and execution.

use thiserror::Error;

use crate::domain::TradeDecision;
use crate::execution::{
    ExecutionEngine, ExecutionError, ExecutionGateway, ExecutionPlan, ExecutionPolicy,
    ExecutionReceipt, MarketQuote,
};
use crate::operator::{OperatorProjectionError, OperatorProjector};
use crate::persistence::RunId;
use crate::risk::{
    RiskConfig, RiskDecision, RiskEngine, RiskOutcome, RiskSnapshot, SafetyState, SafetyStateStore,
};

#[derive(Debug, Error)]
pub enum RuntimeError {
    #[error("execution failed: {0}")]
    Execution(#[from] ExecutionError),
    #[error("operator projection failed: {0}")]
    Projection(#[from] OperatorProjectionError),
}

#[derive(Debug)]
pub enum RuntimeOutcome {
    NoAction {
        risk: RiskDecision,
    },
    Rejected {
        risk: RiskDecision,
    },
    Executed {
        risk: RiskDecision,
        plan: Box<ExecutionPlan>,
        receipt: Box<ExecutionReceipt>,
    },
}

pub struct RuntimeCoordinator<G> {
    risk: RiskEngine,
    policy: ExecutionPolicy,
    execution: ExecutionEngine<G>,
    safety_state: SafetyStateStore,
}

impl<G: ExecutionGateway> RuntimeCoordinator<G> {
    pub fn new(risk_config: RiskConfig, policy: ExecutionPolicy, gateway: G) -> Self {
        Self {
            risk: RiskEngine::new(risk_config),
            policy,
            execution: ExecutionEngine::new(gateway),
            safety_state: SafetyStateStore::new(SafetyState::Normal),
        }
    }

    pub fn with_safety_state_store(
        risk_config: RiskConfig,
        policy: ExecutionPolicy,
        gateway: G,
        safety_state: SafetyStateStore,
    ) -> Self {
        Self {
            risk: RiskEngine::new(risk_config),
            policy,
            execution: ExecutionEngine::new(gateway),
            safety_state,
        }
    }

    pub fn process(
        &mut self,
        decision: TradeDecision,
        snapshot: RiskSnapshot,
        quote: MarketQuote,
    ) -> Result<RuntimeOutcome, RuntimeError> {
        let snapshot = snapshot.with_safety_state(self.safety_state.state());
        let risk = self.risk.evaluate(&decision, &snapshot);
        match risk.outcome {
            RiskOutcome::NoAction => Ok(RuntimeOutcome::NoAction { risk }),
            RiskOutcome::Rejected => Ok(RuntimeOutcome::Rejected { risk }),
            RiskOutcome::Approved | RiskOutcome::ApprovedWithConstraints => {
                let plan = self.policy.plan(&risk, &decision, &quote)?;
                let receipt = self.execution.execute(&risk, plan.clone())?;
                Ok(RuntimeOutcome::Executed {
                    risk,
                    plan: Box::new(plan),
                    receipt: Box::new(receipt),
                })
            }
        }
    }

    pub fn process_with_projection(
        &mut self,
        run_id: RunId,
        decision: TradeDecision,
        snapshot: RiskSnapshot,
        quote: MarketQuote,
        projector: &OperatorProjector,
    ) -> Result<RuntimeOutcome, RuntimeError> {
        let snapshot = snapshot.with_safety_state(self.safety_state.state());
        let risk = self.risk.evaluate(&decision, &snapshot);
        projector.risk_evaluated(run_id, &risk, risk.evaluated_at)?;
        match risk.outcome {
            RiskOutcome::NoAction => Ok(RuntimeOutcome::NoAction { risk }),
            RiskOutcome::Rejected => Ok(RuntimeOutcome::Rejected { risk }),
            RiskOutcome::Approved | RiskOutcome::ApprovedWithConstraints => {
                let plan = self.policy.plan(&risk, &decision, &quote)?;
                projector.execution_planned(run_id, &plan, plan.created_at)?;
                let receipt = match self.execution.execute(&risk, plan.clone()) {
                    Ok(receipt) => receipt,
                    Err(error) => {
                        projector.execution_failed(
                            run_id,
                            &plan,
                            error.to_string(),
                            plan.created_at,
                        )?;
                        return Err(error.into());
                    }
                };
                projector.execution_updated(run_id, &plan, &receipt, plan.created_at)?;
                Ok(RuntimeOutcome::Executed {
                    risk,
                    plan: Box::new(plan),
                    receipt: Box::new(receipt),
                })
            }
        }
    }

    pub fn execution(&self) -> &ExecutionEngine<G> {
        &self.execution
    }

    pub fn execution_mut(&mut self) -> &mut ExecutionEngine<G> {
        &mut self.execution
    }
}
