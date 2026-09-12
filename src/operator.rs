//! Backend-owned operator read model and narrow safety control surface.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex, RwLock};

use serde::{Deserialize, Serialize};
use serde_json::json;
use thiserror::Error;
use time::OffsetDateTime;

use crate::agent_contract::PROFILE_NAME;
use crate::decision::{
    DecisionContext, DecisionCycleResult, DecisionError, DecisionObserver, DecisionOrchestrator,
    DecisionProposal, DecisionProvider, ProposalKind, ResearchBudget, ResearchProvider,
    ResearchSourcePlanner,
};
use crate::domain::{Instrument, PositionState, TradeDecision};
use crate::execution::{ExecutionPlan, ExecutionReceipt, MarketQuote};
use crate::intelligence::sources::SourceError;
use crate::intelligence::{
    IntelligenceEvidence, IntelligenceSnapshot, IntelligenceSource, SourceErrorCategory,
    SourceHealthStatus, SourceHealthView,
};
use crate::persistence::{AuditEvent, AuditStore, RunId, StorageError};
use crate::risk::{OperatingMode, RiskDecision, SafetyState, SafetyStateStore};

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OperatorRunStatus {
    #[default]
    Idle,
    Running,
    Completed,
    NoAction,
    Failed,
    Blocked,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OperatorRunPhase {
    #[default]
    Idle,
    Researching,
    Deciding,
    RiskCheck,
    Executing,
    Reconciling,
    Portfolio,
    Complete,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct OperatorRunView {
    pub run_id: Option<String>,
    pub cycle_id: Option<String>,
    pub status: OperatorRunStatus,
    pub phase: OperatorRunPhase,
    pub started_at: Option<OffsetDateTime>,
    pub updated_at: Option<OffsetDateTime>,
    pub completed_at: Option<OffsetDateTime>,
    pub no_action_reason: Option<String>,
    pub error: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum OperatorAgentStatus {
    Unknown,
    Idle,
    Researching,
    Proposed,
    Degraded,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct OperatorAgentView {
    pub profile: String,
    pub status: OperatorAgentStatus,
    pub schema_version: u16,
    pub run_id: Option<String>,
    pub request_id: Option<String>,
    pub emitted_at: Option<OffsetDateTime>,
    pub decision_id: Option<String>,
    pub last_error: Option<String>,
}

impl Default for OperatorAgentView {
    fn default() -> Self {
        Self {
            profile: PROFILE_NAME.to_owned(),
            status: OperatorAgentStatus::Unknown,
            schema_version: 1,
            run_id: None,
            request_id: None,
            emitted_at: None,
            decision_id: None,
            last_error: None,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct OperatorSourceView {
    pub source: IntelligenceSource,
    pub status: SourceHealthStatus,
    pub last_success: Option<OffsetDateTime>,
    pub last_attempt: Option<OffsetDateTime>,
    pub last_error: Option<SourceErrorCategory>,
    pub consecutive_failures: u32,
    pub signal_count: usize,
}

impl From<SourceHealthView> for OperatorSourceView {
    fn from(value: SourceHealthView) -> Self {
        Self {
            source: value.source,
            status: value.status,
            last_success: value.last_success,
            last_attempt: value.last_attempt,
            last_error: value.last_error,
            consecutive_failures: value.consecutive_failures,
            signal_count: 0,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct OperatorResearchRequest {
    pub source: IntelligenceSource,
    pub instrument: Instrument,
    pub rationale: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct OperatorResearchError {
    pub source: IntelligenceSource,
    pub category: SourceErrorCategory,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct OperatorResearchBudget {
    pub max_source_calls: usize,
    pub max_deep_research_candidates: usize,
    pub max_actionable_decisions: usize,
    pub max_results_per_query: usize,
    pub max_results_per_source: usize,
}

impl From<&ResearchBudget> for OperatorResearchBudget {
    fn from(value: &ResearchBudget) -> Self {
        Self {
            max_source_calls: value.max_source_calls,
            max_deep_research_candidates: value.max_deep_research_candidates,
            max_actionable_decisions: value.max_actionable_decisions,
            max_results_per_query: value.max_results_per_query,
            max_results_per_source: value.max_results_per_source,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct OperatorResearchView {
    pub trigger: Option<String>,
    pub requests: Vec<OperatorResearchRequest>,
    pub errors: Vec<OperatorResearchError>,
    pub budget: Option<OperatorResearchBudget>,
    pub truncated: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct OperatorDecisionProvenance {
    pub schema_version: u16,
    pub profile: String,
    pub request_id: String,
    pub run_id: String,
    pub emitted_at: OffsetDateTime,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct OperatorExecutionView {
    pub plan: ExecutionPlan,
    pub receipt: ExecutionReceipt,
    pub updated_at: OffsetDateTime,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct OperatorOpportunity {
    pub instrument: Instrument,
    pub score: String,
    pub summary: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PortfolioView {
    pub cash: String,
    pub equity: String,
    pub realized_pnl: String,
    pub fees: String,
    pub available_cash: Option<String>,
    pub unrealized_pnl: Option<String>,
    pub pending_orders: usize,
    pub marks: BTreeMap<String, String>,
    pub observed_at: Option<OffsetDateTime>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct OperatorReadModel {
    pub environment: OperatingMode,
    pub safety_state: SafetyState,
    pub market: Vec<MarketQuote>,
    pub run: OperatorRunView,
    pub revision: u64,
    pub updated_at: Option<OffsetDateTime>,
    pub agent: OperatorAgentView,
    pub sources: Vec<OperatorSourceView>,
    pub research: OperatorResearchView,
    pub source_health: BTreeMap<String, String>,
    pub signals: Vec<IntelligenceEvidence>,
    pub opportunities: Vec<OperatorOpportunity>,
    pub positions: Vec<PositionState>,
    pub latest_decision: Option<TradeDecision>,
    pub latest_proposal_kind: Option<String>,
    pub decision_provenance: Option<OperatorDecisionProvenance>,
    pub no_action_reason: Option<String>,
    pub latest_risk_decision: Option<RiskDecision>,
    pub executions: Vec<ExecutionReceipt>,
    pub execution_records: Vec<OperatorExecutionView>,
    pub portfolio: Option<PortfolioView>,
    pub execution_warning: Option<String>,
}

impl Default for OperatorReadModel {
    fn default() -> Self {
        Self {
            environment: OperatingMode::Simulation,
            safety_state: SafetyState::Normal,
            market: Vec::new(),
            run: OperatorRunView::default(),
            revision: 0,
            updated_at: None,
            agent: OperatorAgentView::default(),
            sources: Vec::new(),
            research: OperatorResearchView::default(),
            source_health: BTreeMap::new(),
            signals: Vec::new(),
            opportunities: Vec::new(),
            positions: Vec::new(),
            latest_decision: None,
            latest_proposal_kind: None,
            decision_provenance: None,
            no_action_reason: None,
            latest_risk_decision: None,
            executions: Vec::new(),
            execution_records: Vec::new(),
            portfolio: None,
            execution_warning: None,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct OperatorEvent {
    pub run_id: String,
    pub event_type: String,
    pub entity_id: String,
    pub occurred_at: OffsetDateTime,
    pub payload: serde_json::Value,
}

#[derive(Debug, Error)]
pub enum OperatorProjectionError {
    #[error("operator state lock is unavailable")]
    StateUnavailable,
    #[error("operator projection serialization failed: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("operator projection storage failed: {0}")]
    Storage(#[from] StorageError),
}

#[derive(Clone)]
pub struct OperatorState {
    model: Arc<RwLock<OperatorReadModel>>,
}

impl OperatorState {
    pub fn new(initial: OperatorReadModel) -> Self {
        Self {
            model: Arc::new(RwLock::new(initial)),
        }
    }

    pub fn snapshot(&self) -> OperatorReadModel {
        self.model
            .read()
            .map(|model| model.clone())
            .unwrap_or_default()
    }

    pub fn set_safety_state(&self, state: SafetyState) -> Result<(), OperatorProjectionError> {
        let mut model = self
            .model
            .write()
            .map_err(|_| OperatorProjectionError::StateUnavailable)?;
        model.safety_state = state;
        model.revision = model.revision.saturating_add(1);
        model.updated_at = Some(OffsetDateTime::now_utc());
        Ok(())
    }

    fn replace(&self, model: OperatorReadModel) -> Result<(), OperatorProjectionError> {
        let mut current = self
            .model
            .write()
            .map_err(|_| OperatorProjectionError::StateUnavailable)?;
        *current = model;
        Ok(())
    }
}

#[derive(Clone)]
pub struct OperatorProjector {
    state: OperatorState,
    store: Arc<Mutex<AuditStore>>,
    update_lock: Arc<Mutex<()>>,
}

impl OperatorProjector {
    pub fn new(store: AuditStore, initial: OperatorReadModel) -> Self {
        Self::new_with_state(store, OperatorState::new(initial))
    }

    pub fn new_with_state(store: AuditStore, state: OperatorState) -> Self {
        Self {
            state,
            store: Arc::new(Mutex::new(store)),
            update_lock: Arc::new(Mutex::new(())),
        }
    }

    pub fn state(&self) -> OperatorState {
        self.state.clone()
    }

    pub fn snapshot(&self) -> OperatorReadModel {
        self.state.snapshot()
    }

    pub fn rehydrate(&self) -> Result<bool, OperatorProjectionError> {
        let stored = self
            .store
            .lock()
            .map_err(|_| OperatorProjectionError::StateUnavailable)?
            .load_current_operator_projection()?;
        let Some(stored) = stored else {
            return Ok(false);
        };
        self.state
            .replace(serde_json::from_value(stored.payload)?)?;
        Ok(true)
    }

    pub fn run_started(
        &self,
        run_id: RunId,
        cycle_id: impl Into<String>,
        mode: OperatingMode,
        safety_state: SafetyState,
        at: OffsetDateTime,
    ) -> Result<(), OperatorProjectionError> {
        let run_id_string = run_id.as_uuid().to_string();
        let cycle_id = cycle_id.into();
        let event_payload = json!({
            "run_id": run_id_string,
            "cycle_id": cycle_id,
            "mode": mode,
            "safety_state": safety_state
        });
        self.commit(
            run_id,
            "run_started",
            run_id_string.clone(),
            at,
            event_payload,
            move |model| {
                model.environment = mode;
                model.safety_state = safety_state;
                model.market.clear();
                model.run = OperatorRunView {
                    run_id: Some(run_id_string.clone()),
                    cycle_id: Some(cycle_id.clone()),
                    status: OperatorRunStatus::Running,
                    phase: OperatorRunPhase::Researching,
                    started_at: Some(at),
                    updated_at: Some(at),
                    completed_at: None,
                    no_action_reason: None,
                    error: None,
                };
                model.agent = OperatorAgentView {
                    run_id: Some(run_id_string.clone()),
                    status: OperatorAgentStatus::Researching,
                    ..OperatorAgentView::default()
                };
                model.research = OperatorResearchView::default();
                model.sources.clear();
                model.source_health.clear();
                model.signals.clear();
                model.opportunities.clear();
                model.latest_decision = None;
                model.latest_proposal_kind = None;
                model.decision_provenance = None;
                model.no_action_reason = None;
                model.latest_risk_decision = None;
                model.executions.clear();
                model.execution_records.clear();
                model.positions.clear();
                model.portfolio = None;
                model.execution_warning = None;
            },
        )
    }

    pub fn research_started(
        &self,
        run_id: RunId,
        trigger: impl Into<String>,
        budget: &ResearchBudget,
        at: OffsetDateTime,
    ) -> Result<(), OperatorProjectionError> {
        let trigger = trigger.into();
        let budget = OperatorResearchBudget::from(budget);
        self.commit(
            run_id,
            "research_started",
            run_id.as_uuid().to_string(),
            at,
            json!({"trigger": trigger, "budget": budget}),
            move |model| {
                model.run.phase = OperatorRunPhase::Researching;
                model.run.status = OperatorRunStatus::Running;
                model.run.updated_at = Some(at);
                model.research.trigger = Some(trigger);
                model.research.budget = Some(budget);
                model.agent.status = OperatorAgentStatus::Researching;
            },
        )
    }

    pub fn intelligence_snapshot(
        &self,
        run_id: RunId,
        snapshot: &IntelligenceSnapshot,
        at: OffsetDateTime,
    ) -> Result<(), OperatorProjectionError> {
        let observations = snapshot.observations().to_vec();
        let health = snapshot.source_health().views();
        let errors = snapshot
            .source_errors()
            .iter()
            .map(|error| OperatorResearchError {
                source: error.source(),
                category: error.category(),
            })
            .collect::<Vec<_>>();
        let truncated = snapshot.was_truncated();
        self.commit(
            run_id,
            "intelligence_snapshot",
            run_id.as_uuid().to_string(),
            at,
            json!({"as_of": snapshot.as_of(), "observations": observations, "source_health": health, "source_errors": errors, "truncated": truncated}),
            move |model| {
                merge_signals(&mut model.signals, observations);
                apply_source_health(&mut model.sources, health, &model.signals);
                model.source_health = source_health_map(&model.sources);
                model.research.errors = errors;
                model.research.truncated = truncated;
                model.run.phase = OperatorRunPhase::Deciding;
                model.run.updated_at = Some(at);
            },
        )
    }

    pub fn context_snapshot(
        &self,
        run_id: RunId,
        evidence: &[IntelligenceEvidence],
        source_health: &crate::intelligence::SourceHealthRegistry,
        source_errors: &[SourceError],
        truncated: bool,
        at: OffsetDateTime,
    ) -> Result<(), OperatorProjectionError> {
        let evidence = evidence.to_vec();
        {
            let mut store = self
                .store
                .lock()
                .map_err(|_| OperatorProjectionError::StateUnavailable)?;
            for item in &evidence {
                store.save_evidence(run_id, item)?;
            }
        }
        let health = source_health.views();
        let errors = source_errors
            .iter()
            .map(|error| OperatorResearchError {
                source: error.source(),
                category: error.category(),
            })
            .collect::<Vec<_>>();
        self.commit(
            run_id,
            "context_snapshot",
            run_id.as_uuid().to_string(),
            at,
            json!({"evidence": evidence, "source_health": health, "source_errors": errors, "truncated": truncated}),
            move |model| {
                merge_signals(&mut model.signals, evidence);
                apply_source_health(&mut model.sources, health, &model.signals);
                model.source_health = source_health_map(&model.sources);
                model.research.errors = errors;
                model.research.truncated = truncated;
                model.run.updated_at = Some(at);
            },
        )
    }

    pub fn market_updated(
        &self,
        run_id: RunId,
        quote: MarketQuote,
        at: OffsetDateTime,
    ) -> Result<(), OperatorProjectionError> {
        self.commit(
            run_id,
            "market_snapshot",
            quote.instrument.to_string(),
            at,
            serde_json::to_value(&quote)?,
            move |model| {
                model
                    .market
                    .retain(|current| current.instrument != quote.instrument);
                model.market.push(quote);
                model.run.updated_at = Some(at);
            },
        )
    }

    pub fn research_planned(
        &self,
        run_id: RunId,
        requests: &[crate::decision::ResearchRequest],
        at: OffsetDateTime,
    ) -> Result<(), OperatorProjectionError> {
        let requests = requests
            .iter()
            .map(|request| OperatorResearchRequest {
                source: request.source,
                instrument: request.instrument.clone(),
                rationale: request.rationale.clone(),
            })
            .collect::<Vec<_>>();
        self.commit(
            run_id,
            "research_planned",
            run_id.as_uuid().to_string(),
            at,
            json!({"requests": requests}),
            move |model| {
                model.research.requests = requests;
                model.run.phase = OperatorRunPhase::Researching;
                model.run.updated_at = Some(at);
            },
        )
    }

    pub fn source_completed(
        &self,
        run_id: RunId,
        source: IntelligenceSource,
        evidence: &[IntelligenceEvidence],
        at: OffsetDateTime,
    ) -> Result<(), OperatorProjectionError> {
        let evidence = evidence.to_vec();
        {
            let mut store = self
                .store
                .lock()
                .map_err(|_| OperatorProjectionError::StateUnavailable)?;
            for item in &evidence {
                store.save_evidence(run_id, item)?;
            }
        }
        self.commit(
            run_id,
            "source_completed",
            run_id.as_uuid().to_string(),
            at,
            json!({"source": source, "evidence": evidence}),
            move |model| {
                merge_signals(&mut model.signals, evidence);
                update_source_health(
                    &mut model.sources,
                    source,
                    SourceHealthStatus::Healthy,
                    None,
                    at,
                    &model.signals,
                );
                model.source_health = source_health_map(&model.sources);
                model.run.phase = OperatorRunPhase::Researching;
                model.run.updated_at = Some(at);
            },
        )
    }

    pub fn source_failed(
        &self,
        run_id: RunId,
        error: &SourceError,
        at: OffsetDateTime,
    ) -> Result<(), OperatorProjectionError> {
        let source = error.source();
        let category = error.category();
        self.commit(
            run_id,
            "source_failed",
            run_id.as_uuid().to_string(),
            at,
            json!({"source": source, "category": category}),
            move |model| {
                model
                    .research
                    .errors
                    .push(OperatorResearchError { source, category });
                update_source_health(
                    &mut model.sources,
                    source,
                    source_status_for_error(category),
                    Some(category),
                    at,
                    &model.signals,
                );
                model.source_health = source_health_map(&model.sources);
                model.run.phase = OperatorRunPhase::Researching;
                model.run.updated_at = Some(at);
            },
        )
    }

    pub fn agent_decision_requested(
        &self,
        run_id: RunId,
        at: OffsetDateTime,
    ) -> Result<(), OperatorProjectionError> {
        self.commit(
            run_id,
            "agent_decision_requested",
            run_id.as_uuid().to_string(),
            at,
            json!({"profile": PROFILE_NAME}),
            move |model| {
                model.agent.status = OperatorAgentStatus::Researching;
                model.run.phase = OperatorRunPhase::Deciding;
                model.run.updated_at = Some(at);
            },
        )
    }

    pub fn safety_updated(
        &self,
        run_id: RunId,
        state: SafetyState,
        at: OffsetDateTime,
    ) -> Result<(), OperatorProjectionError> {
        self.commit(
            run_id,
            "safety_updated",
            run_id.as_uuid().to_string(),
            at,
            json!({"safety_state": state}),
            move |model| {
                model.safety_state = state;
                model.run.updated_at = Some(at);
            },
        )
    }

    pub fn decision_cycle(
        &self,
        run_id: RunId,
        cycle: &DecisionCycleResult,
        at: OffsetDateTime,
    ) -> Result<(), OperatorProjectionError> {
        let requests = cycle
            .research_requests()
            .iter()
            .map(|request| OperatorResearchRequest {
                source: request.source,
                instrument: request.instrument.clone(),
                rationale: request.rationale.clone(),
            })
            .collect::<Vec<_>>();
        let errors = cycle
            .research_errors()
            .iter()
            .map(|error| OperatorResearchError {
                source: error.source(),
                category: error.category(),
            })
            .collect::<Vec<_>>();
        let opportunities = cycle
            .ranked_opportunities()
            .iter()
            .map(|ranked| OperatorOpportunity {
                instrument: ranked.ranking.instrument.clone(),
                score: ranked.ranking.opportunity_score.to_string(),
                summary: ranked
                    .supporting_evidence
                    .first()
                    .or_else(|| ranked.counter_evidence.first())
                    .map(|evidence| evidence.summary.clone())
                    .unwrap_or_else(|| "Ranked attention candidate".to_owned()),
            })
            .collect::<Vec<_>>();
        let proposals = cycle.proposals().to_vec();
        let no_action_reason = cycle.no_action_reason().map(str::to_owned);
        self.commit(
            run_id,
            "decision_cycle",
            run_id.as_uuid().to_string(),
            at,
            json!({"requests": requests, "errors": errors, "opportunities": opportunities, "no_action_reason": no_action_reason}),
            move |model| {
                model.run.phase = OperatorRunPhase::Deciding;
                model.run.updated_at = Some(at);
                model.research.requests = requests;
                model.research.errors = errors;
                model.opportunities = opportunities;
                if let Some(reason) = no_action_reason.clone() {
                    model.no_action_reason = Some(reason.clone());
                    model.run.no_action_reason = Some(reason);
                    model.run.status = OperatorRunStatus::NoAction;
                    model.agent.status = OperatorAgentStatus::Degraded;
                }
                if let Some(proposal) = proposals.first() {
                    apply_proposal(model, proposal);
                }
            },
        )
    }

    pub fn risk_evaluated(
        &self,
        run_id: RunId,
        risk: &RiskDecision,
        at: OffsetDateTime,
    ) -> Result<(), OperatorProjectionError> {
        self.commit(
            run_id,
            "risk_evaluated",
            risk.decision_id.as_uuid().to_string(),
            at,
            serde_json::to_value(risk)?,
            move |model| {
                model.latest_risk_decision = Some(risk.clone());
                model.run.phase = OperatorRunPhase::RiskCheck;
                model.run.updated_at = Some(at);
            },
        )
    }

    pub fn execution_updated(
        &self,
        run_id: RunId,
        plan: &ExecutionPlan,
        receipt: &ExecutionReceipt,
        at: OffsetDateTime,
    ) -> Result<(), OperatorProjectionError> {
        let view = OperatorExecutionView {
            plan: plan.clone(),
            receipt: receipt.clone(),
            updated_at: at,
        };
        let unknown = matches!(
            receipt.state,
            crate::execution::ExecutionState::Unknown
                | crate::execution::ExecutionState::CancelPending
        );
        self.commit(
            run_id,
            "execution_updated",
            plan.execution_id.to_string(),
            at,
            serde_json::to_value(&view)?,
            move |model| {
                model.run.phase = if unknown {
                    OperatorRunPhase::Reconciling
                } else {
                    OperatorRunPhase::Executing
                };
                model.run.updated_at = Some(at);
                model
                    .executions
                    .retain(|item| item.execution_id != receipt.execution_id);
                model.executions.push(receipt.clone());
                model
                    .execution_records
                    .retain(|item| item.plan.execution_id != plan.execution_id);
                model.execution_records.push(view);
                if matches!(receipt.state, crate::execution::ExecutionState::Unknown) {
                    model.execution_warning = Some("Execution state unknown. Reconciliation in progress. No duplicate placement will be attempted.".to_owned());
                }
            },
        )
    }

    pub fn execution_planned(
        &self,
        run_id: RunId,
        plan: &ExecutionPlan,
        at: OffsetDateTime,
    ) -> Result<(), OperatorProjectionError> {
        let receipt = ExecutionReceipt {
            execution_id: plan.execution_id,
            client_order_id: plan.client_order_id.clone(),
            exchange_order_id: None,
            state: crate::execution::ExecutionState::Planned,
            filled_quantity: rust_decimal::Decimal::ZERO,
            remaining_quantity: match plan.size {
                crate::execution::PlannedSize::Quantity(quantity) => Some(quantity),
                crate::execution::PlannedSize::Notional(_) => None,
            },
            average_price: None,
        };
        self.execution_updated(run_id, plan, &receipt, at)
    }

    pub fn execution_failed(
        &self,
        run_id: RunId,
        plan: &ExecutionPlan,
        error: impl Into<String>,
        at: OffsetDateTime,
    ) -> Result<(), OperatorProjectionError> {
        let error = error.into();
        self.commit(
            run_id,
            "execution_failed",
            plan.execution_id.to_string(),
            at,
            json!({"execution_id": plan.execution_id, "error": error}),
            move |model| {
                model.run.phase = OperatorRunPhase::Reconciling;
                model.run.status = OperatorRunStatus::Failed;
                model.run.error = Some(error.clone());
                model.run.updated_at = Some(at);
                model.execution_warning = Some(error);
            },
        )
    }

    pub fn portfolio_updated(
        &self,
        run_id: RunId,
        portfolio: PortfolioView,
        positions: Vec<PositionState>,
        at: OffsetDateTime,
    ) -> Result<(), OperatorProjectionError> {
        self.commit(
            run_id,
            "portfolio_updated",
            run_id.as_uuid().to_string(),
            at,
            json!({"portfolio": &portfolio, "positions": &positions}),
            move |model| {
                model.portfolio = Some(portfolio);
                model.positions = positions;
                model.run.phase = OperatorRunPhase::Portfolio;
                model.run.updated_at = Some(at);
            },
        )
    }

    pub fn run_completed(
        &self,
        run_id: RunId,
        at: OffsetDateTime,
    ) -> Result<(), OperatorProjectionError> {
        self.commit(
            run_id,
            "run_completed",
            run_id.as_uuid().to_string(),
            at,
            json!({"status": "completed"}),
            move |model| {
                model.run.status = OperatorRunStatus::Completed;
                model.run.phase = OperatorRunPhase::Complete;
                model.run.updated_at = Some(at);
                model.run.completed_at = Some(at);
                model.agent.status = OperatorAgentStatus::Idle;
            },
        )
    }

    pub fn run_failed(
        &self,
        run_id: RunId,
        error: impl Into<String>,
        at: OffsetDateTime,
    ) -> Result<(), OperatorProjectionError> {
        let error = error.into();
        self.commit(
            run_id,
            "run_failed",
            run_id.as_uuid().to_string(),
            at,
            json!({"error": error}),
            move |model| {
                model.run.status = OperatorRunStatus::Failed;
                model.run.error = Some(error.clone());
                model.run.updated_at = Some(at);
                model.run.completed_at = Some(at);
                model.agent.status = OperatorAgentStatus::Degraded;
                model.agent.last_error = Some(error);
            },
        )
    }

    fn commit(
        &self,
        run_id: RunId,
        event_type: &str,
        entity_id: String,
        at: OffsetDateTime,
        event_payload: serde_json::Value,
        update: impl FnOnce(&mut OperatorReadModel),
    ) -> Result<(), OperatorProjectionError> {
        let _update_guard = self
            .update_lock
            .lock()
            .map_err(|_| OperatorProjectionError::StateUnavailable)?;
        let mut next = self.state.snapshot();
        update(&mut next);
        next.updated_at = Some(at);
        next.revision = next.revision.saturating_add(1);
        let projection_payload = serde_json::to_value(&next)?;
        self.store
            .lock()
            .map_err(|_| OperatorProjectionError::StateUnavailable)?
            .append_event_with_projection(
                AuditEvent::new(run_id, event_type, entity_id, at, event_payload),
                next.revision,
                projection_payload,
            )?;
        self.state.replace(next)?;
        Ok(())
    }
}

fn apply_proposal(model: &mut OperatorReadModel, proposal: &DecisionProposal) {
    let decision = proposal.decision.decision().clone();
    let provenance = proposal.decision.provenance();
    model.latest_decision = Some(decision.clone());
    model.latest_proposal_kind = Some(proposal_kind_label(proposal.kind).to_owned());
    model.decision_provenance = Some(OperatorDecisionProvenance {
        schema_version: provenance.schema_version,
        profile: provenance.profile.clone(),
        request_id: provenance.request_id.clone(),
        run_id: provenance.run_id.clone(),
        emitted_at: provenance.emitted_at,
    });
    model.agent.status = OperatorAgentStatus::Proposed;
    model.agent.request_id = Some(provenance.request_id.clone());
    model.agent.run_id = Some(provenance.run_id.clone());
    model.agent.emitted_at = Some(provenance.emitted_at);
    model.agent.decision_id = Some(decision.decision_id.as_uuid().to_string());
    model.run.phase = OperatorRunPhase::RiskCheck;
}

fn merge_signals(current: &mut Vec<IntelligenceEvidence>, incoming: Vec<IntelligenceEvidence>) {
    for signal in incoming {
        if let Some(existing) = current
            .iter_mut()
            .find(|existing| existing.evidence_id == signal.evidence_id)
        {
            *existing = signal;
        } else {
            current.push(signal);
        }
    }
    current.sort_by(|left, right| {
        right
            .observed_at
            .cmp(&left.observed_at)
            .then_with(|| left.evidence_id.cmp(&right.evidence_id))
    });
}

fn apply_source_health(
    current: &mut Vec<OperatorSourceView>,
    incoming: Vec<SourceHealthView>,
    signals: &[IntelligenceEvidence],
) {
    let mut views = incoming
        .into_iter()
        .map(OperatorSourceView::from)
        .collect::<Vec<_>>();
    for view in &mut views {
        view.signal_count = signals
            .iter()
            .filter(|signal| signal.source == view.source)
            .count();
    }
    *current = views;
}

fn update_source_health(
    sources: &mut Vec<OperatorSourceView>,
    source: IntelligenceSource,
    status: SourceHealthStatus,
    error: Option<SourceErrorCategory>,
    at: OffsetDateTime,
    signals: &[IntelligenceEvidence],
) {
    let view = sources.iter_mut().find(|view| view.source == source);
    if let Some(view) = view {
        view.status = status;
        view.last_attempt = Some(at);
        view.last_error = error;
        if status == SourceHealthStatus::Healthy {
            view.last_success = Some(at);
            view.consecutive_failures = 0;
        } else {
            view.consecutive_failures = view.consecutive_failures.saturating_add(1);
        }
        view.signal_count = signals
            .iter()
            .filter(|signal| signal.source == source)
            .count();
        return;
    }
    sources.push(OperatorSourceView {
        source,
        status,
        last_success: (status == SourceHealthStatus::Healthy).then_some(at),
        last_attempt: Some(at),
        last_error: error,
        consecutive_failures: if status == SourceHealthStatus::Healthy {
            0
        } else {
            1
        },
        signal_count: signals
            .iter()
            .filter(|signal| signal.source == source)
            .count(),
    });
}

fn source_status_for_error(category: SourceErrorCategory) -> SourceHealthStatus {
    match category {
        SourceErrorCategory::Unavailable
        | SourceErrorCategory::Unauthorized
        | SourceErrorCategory::CapabilityMissing
        | SourceErrorCategory::ProcessFailure => SourceHealthStatus::Unavailable,
        SourceErrorCategory::Disabled => SourceHealthStatus::Disabled,
        _ => SourceHealthStatus::Degraded,
    }
}

fn source_health_map(sources: &[OperatorSourceView]) -> BTreeMap<String, String> {
    sources
        .iter()
        .map(|source| {
            (
                source_key(source.source).to_owned(),
                source_status_label(source.status).to_owned(),
            )
        })
        .collect()
}

fn source_key(source: IntelligenceSource) -> &'static str {
    match source {
        IntelligenceSource::AtkNews => "atk_news",
        IntelligenceSource::Polymarket => "polymarket",
        IntelligenceSource::Reddit => "reddit",
        IntelligenceSource::MarxFinance => "marx_finance",
    }
}

fn source_status_label(status: SourceHealthStatus) -> &'static str {
    match status {
        SourceHealthStatus::Unknown => "unknown",
        SourceHealthStatus::Healthy => "healthy",
        SourceHealthStatus::Degraded => "degraded",
        SourceHealthStatus::Unavailable => "unavailable",
        SourceHealthStatus::Disabled => "disabled",
    }
}

fn proposal_kind_label(kind: ProposalKind) -> &'static str {
    match kind {
        ProposalKind::Entry => "entry",
        ProposalKind::Add => "add",
        ProposalKind::Reduction => "reduction",
        ProposalKind::Hold => "hold",
    }
}

pub struct OperatorDecisionObserver {
    projector: OperatorProjector,
    run_id: RunId,
}

impl OperatorDecisionObserver {
    pub fn new(projector: OperatorProjector, run_id: RunId) -> Self {
        Self { projector, run_id }
    }
}

impl DecisionObserver for OperatorDecisionObserver {
    type Error = OperatorProjectionError;

    fn on_research_planned(
        &mut self,
        requests: &[crate::decision::ResearchRequest],
        at: OffsetDateTime,
    ) -> Result<(), Self::Error> {
        self.projector.research_planned(self.run_id, requests, at)
    }

    fn on_source_completed(
        &mut self,
        source: IntelligenceSource,
        evidence: &[IntelligenceEvidence],
        at: OffsetDateTime,
    ) -> Result<(), Self::Error> {
        self.projector
            .source_completed(self.run_id, source, evidence, at)
    }

    fn on_source_failed(
        &mut self,
        error: &SourceError,
        at: OffsetDateTime,
    ) -> Result<(), Self::Error> {
        self.projector.source_failed(self.run_id, error, at)
    }

    fn on_agent_decision_requested(&mut self, at: OffsetDateTime) -> Result<(), Self::Error> {
        self.projector.agent_decision_requested(self.run_id, at)
    }
}

pub fn run_decision_cycle<P, R, D>(
    orchestrator: &mut DecisionOrchestrator<P, R, D>,
    context: DecisionContext,
    run_id: RunId,
    cycle_id: impl Into<String>,
    mode: OperatingMode,
    safety_state: SafetyState,
    projector: &OperatorProjector,
) -> Result<DecisionCycleResult, DecisionError>
where
    P: ResearchSourcePlanner,
    R: ResearchProvider,
    D: DecisionProvider,
{
    let at = context.as_of;
    let cycle_id = cycle_id.into();
    let trigger = format!("{:?}", context.trigger);
    projector
        .run_started(run_id, cycle_id, mode, safety_state, at)
        .map_err(|error| DecisionError::Observer(error.to_string()))?;
    projector
        .context_snapshot(
            run_id,
            &context.evidence,
            &context.source_health,
            &context.source_errors,
            false,
            at,
        )
        .map_err(|error| DecisionError::Observer(error.to_string()))?;
    projector
        .research_started(run_id, trigger, orchestrator.budget(), at)
        .map_err(|error| DecisionError::Observer(error.to_string()))?;
    let mut observer = OperatorDecisionObserver::new(projector.clone(), run_id);
    let result = orchestrator.run_with_observer(context, &mut observer);
    match result {
        Ok(cycle) => {
            projector
                .decision_cycle(run_id, &cycle, at)
                .map_err(|error| DecisionError::Observer(error.to_string()))?;
            Ok(cycle)
        }
        Err(error) => {
            let _ = projector.run_failed(run_id, error.to_string(), at);
            Err(error)
        }
    }
}

#[derive(Debug, Error)]
pub enum OperatorError {
    #[error("explicit KILL confirmation is required")]
    ConfirmationRequired,
    #[error("operator state lock is unavailable")]
    StateUnavailable,
}

#[derive(Clone)]
pub struct OperatorController {
    safety_state: SafetyStateStore,
    state: OperatorState,
    projector: Option<OperatorProjector>,
}

impl OperatorController {
    pub fn new(initial: SafetyState) -> Self {
        let model = OperatorReadModel {
            safety_state: initial,
            ..Default::default()
        };
        Self::with_state(OperatorState::new(model), SafetyStateStore::new(initial))
    }

    pub fn with_state(state: OperatorState, safety_state: SafetyStateStore) -> Self {
        Self {
            safety_state,
            state,
            projector: None,
        }
    }

    pub fn with_projector(projector: OperatorProjector, safety_state: SafetyStateStore) -> Self {
        Self {
            state: projector.state(),
            safety_state,
            projector: Some(projector),
        }
    }

    pub fn with_persistent_store(
        initial: OperatorReadModel,
        safety_state: SafetyStateStore,
        store: AuditStore,
    ) -> Self {
        let state = OperatorState::new(initial);
        let projector = OperatorProjector::new_with_state(store, state.clone());
        let _ = projector.rehydrate();
        let persisted_safety = state.snapshot().safety_state;
        if safety_state.state() == SafetyState::Normal && persisted_safety != SafetyState::Normal {
            let _ = safety_state.set(persisted_safety);
        }
        Self {
            state,
            safety_state,
            projector: Some(projector),
        }
    }

    pub fn safety_state_store(&self) -> SafetyStateStore {
        self.safety_state.clone()
    }

    pub fn operator_state(&self) -> OperatorState {
        self.state.clone()
    }

    pub fn state(&self) -> SafetyState {
        self.safety_state.state()
    }

    pub fn kill_switch(&self, confirmation: &str) -> Result<SafetyState, OperatorError> {
        if confirmation != "KILL" {
            return Err(OperatorError::ConfirmationRequired);
        }
        self.safety_state
            .kill()
            .map_err(|_| OperatorError::StateUnavailable)?;
        self.state
            .set_safety_state(SafetyState::Killed)
            .map_err(|_| OperatorError::StateUnavailable)?;
        if let Some(projector) = &self.projector
            && let Some(run_id) = self
                .state
                .snapshot()
                .run
                .run_id
                .as_deref()
                .and_then(|value| uuid::Uuid::parse_str(value).ok())
        {
            projector
                .safety_updated(
                    RunId::from_uuid(run_id),
                    SafetyState::Killed,
                    OffsetDateTime::now_utc(),
                )
                .map_err(|_| OperatorError::StateUnavailable)?;
        }
        Ok(SafetyState::Killed)
    }

    pub fn handle(
        &self,
        method: &str,
        path: &str,
        body: &str,
        model: &OperatorReadModel,
    ) -> OperatorResponse {
        match (method, path) {
            ("GET", "/") => {
                let mut view = model.clone();
                view.safety_state = self.state();
                OperatorResponse::html(200, render_dashboard(&view))
            }
            ("GET", "/api/read-model") => {
                let safety_state = self.state();
                let mut view = model.clone();
                view.safety_state = safety_state;
                let payload = json!({
                    "environment": environment_label(view.environment),
                    "safety_state": safety_label(safety_state),
                    "revision": view.revision,
                    "updated_at": view.updated_at,
                    "model": view,
                });
                OperatorResponse::json(200, payload)
            }
            ("POST", "/api/safety/kill") => match self.kill_switch(body) {
                Ok(state) => OperatorResponse::json(
                    200,
                    json!({"safety_state": safety_label(state), "message": "new risk is disabled"}),
                ),
                Err(error) => OperatorResponse::json(400, json!({"error": error.to_string()})),
            },
            _ => OperatorResponse::json(404, json!({"error": "operator route not found"})),
        }
    }

    pub fn handle_current(&self, method: &str, path: &str, body: &str) -> OperatorResponse {
        if let Some(projector) = &self.projector {
            let _ = projector.rehydrate();
        }
        let model = self.state.snapshot();
        self.handle(method, path, body, &model)
    }

    pub fn serve_once(
        &self,
        listener: TcpListener,
        model: &OperatorReadModel,
    ) -> std::io::Result<()> {
        let (mut stream, _) = listener.accept()?;
        let mut buffer = [0_u8; 8 * 1024];
        let length = stream.read(&mut buffer)?;
        let request = String::from_utf8_lossy(&buffer[..length]);
        let mut lines = request.split("\r\n");
        let request_line = lines.next().unwrap_or_default();
        let mut parts = request_line.split_whitespace();
        let method = parts.next().unwrap_or("GET");
        let path = parts.next().unwrap_or("/");
        let body = request
            .split_once("\r\n\r\n")
            .map(|(_, body)| body)
            .unwrap_or_default();
        let response = self.handle(method, path, body.trim(), model);
        let status_text = match response.status {
            200 => "OK",
            400 => "Bad Request",
            404 => "Not Found",
            _ => "Response",
        };
        write!(
            stream,
            "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            response.status,
            status_text,
            response.content_type,
            response.body.len(),
            response.body
        )?;
        Ok(())
    }

    pub fn serve(&self, listener: TcpListener) -> std::io::Result<()> {
        for stream in listener.incoming() {
            let mut stream = stream?;
            let mut buffer = [0_u8; 8 * 1024];
            let length = stream.read(&mut buffer)?;
            let request = String::from_utf8_lossy(&buffer[..length]);
            let mut lines = request.split("\r\n");
            let request_line = lines.next().unwrap_or_default();
            let mut parts = request_line.split_whitespace();
            let method = parts.next().unwrap_or("GET");
            let path = parts.next().unwrap_or("/");
            let body = request
                .split_once("\r\n\r\n")
                .map(|(_, body)| body)
                .unwrap_or_default();
            let response = self.handle_current(method, path, body.trim());
            write_response(&mut stream, &response)?;
        }
        Ok(())
    }
}

fn write_response(stream: &mut impl Write, response: &OperatorResponse) -> std::io::Result<()> {
    let status_text = match response.status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        _ => "Response",
    };
    write!(
        stream,
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        response.status,
        status_text,
        response.content_type,
        response.body.len(),
        response.body
    )
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OperatorResponse {
    pub status: u16,
    pub content_type: &'static str,
    pub body: String,
}

impl OperatorResponse {
    fn json(status: u16, payload: serde_json::Value) -> Self {
        Self {
            status,
            content_type: "application/json",
            body: payload.to_string(),
        }
    }

    fn html(status: u16, body: String) -> Self {
        Self {
            status,
            content_type: "text/html; charset=utf-8",
            body,
        }
    }
}

pub fn render_dashboard(model: &OperatorReadModel) -> String {
    let payload = json!({
        "environment": environment_label(model.environment),
        "safety_state": safety_label(model.safety_state),
        "revision": model.revision,
        "updated_at": model.updated_at,
        "model": model,
    });
    let initial_json = serde_json::to_string(&payload)
        .unwrap_or_else(|_| "null".to_owned())
        .replace('<', "\\u003c")
        .replace('>', "\\u003e")
        .replace('&', "\\u0026");
    include_str!("../ui/index.html").replace(
        "window.__PROOFTRADE_INITIAL__ = null;",
        &format!("window.__PROOFTRADE_INITIAL__ = {initial_json};"),
    )
}

fn environment_label(environment: OperatingMode) -> &'static str {
    match environment {
        OperatingMode::Simulation => "SIMULATION",
        OperatingMode::Demo => "DEMO",
        OperatingMode::Live => "LIVE",
    }
}

fn safety_label(state: SafetyState) -> &'static str {
    match state {
        SafetyState::Normal => "NORMAL",
        SafetyState::Safe => "SAFE",
        SafetyState::Killed => "KILLED",
    }
}
