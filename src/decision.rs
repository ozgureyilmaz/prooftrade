//! Deterministic coordination around non-deterministic agent reasoning.
//!
//! This module ranks attention, bounds targeted intelligence research, builds a
//! typed request for Hermes, and validates the returned proposed intents. It
//! never approves capital, submits orders, or executes the simulator.

use std::collections::{BTreeMap, BTreeSet};

use rust_decimal::Decimal;
use thiserror::Error;
use time::OffsetDateTime;

use crate::domain::{
    DecisionId, DomainError, EvidenceId, Instrument, PositionState, RankedOpportunity, SourceKind,
    TradeAction, TradeDecision, TradingUniverse, rank_opportunities,
};
use crate::hermes::{ValidatedAgentDecision, decode_agent_decision};
use crate::intelligence::sources::SourceError;
use crate::intelligence::{
    EventFingerprint, IntelligenceDirection, IntelligenceEvidence, IntelligenceService,
    IntelligenceSnapshot, IntelligenceSource, SourceErrorCategory, SourceHealthRegistry,
};

#[derive(Debug, Error, Eq, PartialEq)]
pub enum DecisionError {
    #[error("invalid decision context: {0}")]
    InvalidContext(String),
    #[error("opportunity ranking failed: {0}")]
    Ranking(#[from] DomainError),
    #[error("decision observer failed: {0}")]
    Observer(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DecisionTrigger {
    OpportunityScan {
        reason: String,
    },
    EventTriggered {
        fingerprint: EventFingerprint,
        instruments: Vec<Instrument>,
    },
    PositionReview {
        instrument: Instrument,
        reason: String,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MarketContext {
    pub instrument: Instrument,
    pub observed_at: OffsetDateTime,
    pub bid: Decimal,
    pub ask: Decimal,
    pub last: Decimal,
    pub recent_move_pct: Option<Decimal>,
    pub volume: Option<Decimal>,
    pub volatility: Option<Decimal>,
}

impl MarketContext {
    pub fn new(
        instrument: Instrument,
        observed_at: OffsetDateTime,
        bid: Decimal,
        ask: Decimal,
        last: Decimal,
    ) -> Result<Self, DecisionError> {
        if bid <= Decimal::ZERO || ask <= Decimal::ZERO || last <= Decimal::ZERO {
            return Err(DecisionError::InvalidContext(
                "market prices must be greater than zero".to_owned(),
            ));
        }
        if bid > ask {
            return Err(DecisionError::InvalidContext(
                "market bid cannot exceed ask".to_owned(),
            ));
        }
        instrument
            .validate()
            .map_err(|error| DecisionError::InvalidContext(error.to_string()))?;
        Ok(Self {
            instrument,
            observed_at,
            bid,
            ask,
            last,
            recent_move_pct: None,
            volume: None,
            volatility: None,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PositionSnapshot {
    as_of: OffsetDateTime,
    positions: Vec<PositionState>,
}

impl PositionSnapshot {
    pub fn new(
        as_of: OffsetDateTime,
        positions: Vec<PositionState>,
    ) -> Result<Self, DecisionError> {
        let mut instruments = BTreeSet::new();
        for position in &positions {
            position
                .validate()
                .map_err(|error| DecisionError::InvalidContext(error.to_string()))?;
            if !instruments.insert(position.instrument().clone()) {
                return Err(DecisionError::InvalidContext(format!(
                    "duplicate position instrument: {}",
                    position.instrument()
                )));
            }
        }
        Ok(Self { as_of, positions })
    }

    pub fn as_of(&self) -> OffsetDateTime {
        self.as_of
    }

    pub fn positions(&self) -> &[PositionState] {
        &self.positions
    }

    pub fn for_instrument(&self, instrument: &Instrument) -> Option<&PositionState> {
        self.positions
            .iter()
            .find(|position| position.instrument() == instrument)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PriorDecision {
    pub decision_id: DecisionId,
    pub instrument: Instrument,
    pub action: TradeAction,
    pub evidence_ids: BTreeSet<EvidenceId>,
}

impl PriorDecision {
    pub fn new(
        decision_id: DecisionId,
        instrument: Instrument,
        action: TradeAction,
        evidence_ids: impl IntoIterator<Item = EvidenceId>,
    ) -> Self {
        Self {
            decision_id,
            instrument,
            action,
            evidence_ids: evidence_ids.into_iter().collect(),
        }
    }

    fn matches(&self, decision: &ValidatedAgentDecision) -> bool {
        let proposed = decision.decision();
        self.instrument == proposed.instrument
            && self.action == proposed.action
            && self.evidence_ids
                == proposed
                    .evidence_refs
                    .iter()
                    .map(|reference| reference.evidence_id)
                    .collect()
    }
}

#[derive(Clone, Debug)]
pub struct DecisionContext {
    pub trigger: DecisionTrigger,
    pub universe: TradingUniverse,
    pub as_of: OffsetDateTime,
    pub evidence: Vec<IntelligenceEvidence>,
    market_context: BTreeMap<Instrument, MarketContext>,
    pub positions: PositionSnapshot,
    pub source_health: SourceHealthRegistry,
    pub source_errors: Vec<SourceError>,
    pub prior_decisions: Vec<PriorDecision>,
}

impl DecisionContext {
    pub fn new(
        trigger: DecisionTrigger,
        universe: TradingUniverse,
        as_of: OffsetDateTime,
        evidence: Vec<IntelligenceEvidence>,
        market_context: Vec<MarketContext>,
        positions: PositionSnapshot,
        source_health: SourceHealthRegistry,
    ) -> Result<Self, DecisionError> {
        universe
            .validate()
            .map_err(|error| DecisionError::InvalidContext(error.to_string()))?;
        for item in &evidence {
            item.validate()
                .map_err(|error| DecisionError::InvalidContext(error.to_string()))?;
            for impact in &item.affected_assets {
                if !universe.contains(&impact.instrument) {
                    return Err(DecisionError::InvalidContext(format!(
                        "evidence references instrument outside universe: {}",
                        impact.instrument
                    )));
                }
            }
        }
        for position in positions.positions() {
            if !universe.contains(position.instrument()) {
                return Err(DecisionError::InvalidContext(format!(
                    "position is outside configured universe: {}",
                    position.instrument()
                )));
            }
        }
        let market_context = market_map(market_context)?;
        Ok(Self {
            trigger,
            universe,
            as_of,
            evidence,
            market_context,
            positions,
            source_health,
            source_errors: Vec::new(),
            prior_decisions: Vec::new(),
        })
    }

    pub fn from_intelligence_snapshot(
        trigger: DecisionTrigger,
        universe: TradingUniverse,
        snapshot: &IntelligenceSnapshot,
        market_context: Vec<MarketContext>,
        positions: PositionSnapshot,
    ) -> Result<Self, DecisionError> {
        let mut context = Self::new(
            trigger,
            universe,
            snapshot.as_of(),
            snapshot.observations().to_vec(),
            market_context,
            positions,
            snapshot.source_health().clone(),
        )?;
        context.source_errors = snapshot.source_errors().to_vec();
        Ok(context)
    }

    pub fn market_context(&self) -> &BTreeMap<Instrument, MarketContext> {
        &self.market_context
    }

    pub fn with_prior_decisions(mut self, prior_decisions: Vec<PriorDecision>) -> Self {
        self.prior_decisions = prior_decisions;
        self
    }

    pub fn set_market_context(
        &mut self,
        market_context: Vec<MarketContext>,
    ) -> Result<(), DecisionError> {
        self.market_context = market_map(market_context)?;
        Ok(())
    }
}

fn market_map(
    market_context: Vec<MarketContext>,
) -> Result<BTreeMap<Instrument, MarketContext>, DecisionError> {
    let mut mapped = BTreeMap::new();
    for market in market_context {
        if mapped.insert(market.instrument.clone(), market).is_some() {
            return Err(DecisionError::InvalidContext(
                "duplicate market instrument".to_owned(),
            ));
        }
    }
    Ok(mapped)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResearchBudget {
    pub max_source_calls: usize,
    pub max_deep_research_candidates: usize,
    pub max_actionable_decisions: usize,
    pub max_results_per_query: usize,
    pub max_results_per_source: usize,
}

impl Default for ResearchBudget {
    fn default() -> Self {
        Self {
            max_source_calls: 8,
            max_deep_research_candidates: 3,
            max_actionable_decisions: 3,
            max_results_per_query: 20,
            max_results_per_source: 10,
        }
    }
}

impl ResearchBudget {
    pub fn with_max_source_calls(mut self, value: usize) -> Self {
        self.max_source_calls = value;
        self
    }

    pub fn with_max_deep_research_candidates(mut self, value: usize) -> Self {
        self.max_deep_research_candidates = value;
        self
    }

    pub fn with_max_actionable_decisions(mut self, value: usize) -> Self {
        self.max_actionable_decisions = value;
        self
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResearchRequest {
    pub source: IntelligenceSource,
    pub instrument: Instrument,
    pub query: crate::intelligence::BoundedEvidenceQuery,
    pub rationale: String,
}

impl ResearchRequest {
    pub fn new(
        source: IntelligenceSource,
        instrument: Instrument,
        query: crate::intelligence::BoundedEvidenceQuery,
        rationale: impl Into<String>,
    ) -> Self {
        Self {
            source,
            instrument,
            query,
            rationale: rationale.into(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResearchResult {
    pub source: IntelligenceSource,
    pub evidence: Vec<IntelligenceEvidence>,
}

impl ResearchResult {
    pub fn new(source: IntelligenceSource, evidence: Vec<IntelligenceEvidence>) -> Self {
        Self { source, evidence }
    }
}

pub trait ResearchSourcePlanner {
    type Error: std::fmt::Debug;

    fn plan(
        &mut self,
        context: &ResearchPlanningContext,
    ) -> Result<Vec<ResearchRequest>, Self::Error>;
}

pub trait ResearchProvider {
    fn research(&mut self, request: &ResearchRequest) -> Result<ResearchResult, SourceError>;
}

impl ResearchProvider for IntelligenceService {
    fn research(&mut self, request: &ResearchRequest) -> Result<ResearchResult, SourceError> {
        let snapshot = self
            .collect_source(request.source, &request.query)
            .map_err(|_| {
                SourceError::new(request.source, SourceErrorCategory::MalformedResponse)
            })?;
        if snapshot.disabled_sources().contains(&request.source) {
            return Err(SourceError::new(
                request.source,
                SourceErrorCategory::Disabled,
            ));
        }
        if let Some(error) = snapshot
            .source_errors()
            .iter()
            .find(|error| error.source() == request.source)
        {
            return Err(error.clone());
        }
        Ok(ResearchResult::new(
            request.source,
            snapshot
                .observations()
                .iter()
                .filter(|evidence| evidence.source == request.source)
                .cloned()
                .collect(),
        ))
    }
}

pub trait DecisionProvider {
    type Error: std::fmt::Debug;

    fn decide(&mut self, request: &AgentDecisionRequest) -> Result<Vec<String>, Self::Error>;
}

pub trait DecisionObserver {
    type Error: std::fmt::Display;

    fn on_research_planned(
        &mut self,
        _requests: &[ResearchRequest],
        _at: OffsetDateTime,
    ) -> Result<(), Self::Error> {
        Ok(())
    }

    fn on_source_completed(
        &mut self,
        _source: IntelligenceSource,
        _evidence: &[IntelligenceEvidence],
        _at: OffsetDateTime,
    ) -> Result<(), Self::Error> {
        Ok(())
    }

    fn on_source_failed(
        &mut self,
        _error: &SourceError,
        _at: OffsetDateTime,
    ) -> Result<(), Self::Error> {
        Ok(())
    }

    fn on_agent_decision_requested(&mut self, _at: OffsetDateTime) -> Result<(), Self::Error> {
        Ok(())
    }
}

struct NoopDecisionObserver;

impl DecisionObserver for NoopDecisionObserver {
    type Error = std::convert::Infallible;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RankedOpportunityContext {
    pub ranking: RankedOpportunity,
    pub supporting_evidence: Vec<IntelligenceEvidence>,
    pub counter_evidence: Vec<IntelligenceEvidence>,
    pub other_evidence: Vec<IntelligenceEvidence>,
    pub event_lineages: BTreeSet<EventFingerprint>,
    pub new_evidence_ids: BTreeSet<EvidenceId>,
}

#[derive(Clone, Debug)]
pub struct ResearchPlanningContext {
    pub context: DecisionContext,
    pub ranked_opportunities: Vec<RankedOpportunityContext>,
    pub budget: ResearchBudget,
}

#[derive(Clone, Debug)]
pub struct AgentDecisionRequest {
    pub context: DecisionContext,
    pub ranked_opportunities: Vec<RankedOpportunityContext>,
    pub research_requests: Vec<ResearchRequest>,
    pub research_errors: Vec<SourceError>,
    pub budget: ResearchBudget,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProposalKind {
    Entry,
    Add,
    Reduction,
    Hold,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DecisionProposal {
    pub decision: ValidatedAgentDecision,
    pub kind: ProposalKind,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DecisionCycleResult {
    Completed {
        ranked_opportunities: Vec<RankedOpportunityContext>,
        proposals: Vec<DecisionProposal>,
        research_requests: Vec<ResearchRequest>,
        research_errors: Vec<SourceError>,
    },
    NoAction {
        reason: String,
        ranked_opportunities: Vec<RankedOpportunityContext>,
        research_requests: Vec<ResearchRequest>,
        research_errors: Vec<SourceError>,
    },
}

impl DecisionCycleResult {
    pub fn ranked_opportunities(&self) -> &[RankedOpportunityContext] {
        match self {
            Self::Completed {
                ranked_opportunities,
                ..
            }
            | Self::NoAction {
                ranked_opportunities,
                ..
            } => ranked_opportunities,
        }
    }

    pub fn proposals(&self) -> &[DecisionProposal] {
        match self {
            Self::Completed { proposals, .. } => proposals,
            Self::NoAction { .. } => &[],
        }
    }

    pub fn research_requests(&self) -> &[ResearchRequest] {
        match self {
            Self::Completed {
                research_requests, ..
            }
            | Self::NoAction {
                research_requests, ..
            } => research_requests,
        }
    }

    pub fn research_errors(&self) -> &[SourceError] {
        match self {
            Self::Completed {
                research_errors, ..
            }
            | Self::NoAction {
                research_errors, ..
            } => research_errors,
        }
    }

    pub fn no_action_reason(&self) -> Option<&str> {
        match self {
            Self::NoAction { reason, .. } => Some(reason),
            Self::Completed { .. } => None,
        }
    }
}

pub struct DecisionOrchestrator<P, R, D> {
    planner: P,
    researcher: R,
    decision_provider: D,
    budget: ResearchBudget,
}

impl<P, D> DecisionOrchestrator<P, IntelligenceService, D>
where
    P: ResearchSourcePlanner,
    D: DecisionProvider,
{
    pub fn enable_polymarket(&mut self) -> bool {
        self.researcher.enable_polymarket()
    }

    pub fn disable_polymarket(&mut self) -> bool {
        self.researcher.disable_polymarket()
    }

    pub fn polymarket_enabled(&self) -> bool {
        self.researcher
            .is_source_enabled(IntelligenceSource::Polymarket)
    }
}

impl<P, R, D> DecisionOrchestrator<P, R, D> {
    pub fn new(planner: P, researcher: R, decision_provider: D, budget: ResearchBudget) -> Self {
        Self {
            planner,
            researcher,
            decision_provider,
            budget,
        }
    }

    pub fn with_budget(mut self, budget: ResearchBudget) -> Self {
        self.budget = budget;
        self
    }

    pub fn budget(&self) -> &ResearchBudget {
        &self.budget
    }
}

impl<P, R, D> DecisionOrchestrator<P, R, D>
where
    P: ResearchSourcePlanner,
    R: ResearchProvider,
    D: DecisionProvider,
{
    pub fn run(&mut self, context: DecisionContext) -> Result<DecisionCycleResult, DecisionError> {
        let mut observer = NoopDecisionObserver;
        self.run_with_observer(context, &mut observer)
    }

    pub fn run_with_observer<O>(
        &mut self,
        context: DecisionContext,
        observer: &mut O,
    ) -> Result<DecisionCycleResult, DecisionError>
    where
        O: DecisionObserver,
    {
        let initial_ranked = rank_context(&context)?;
        let planning_context = ResearchPlanningContext {
            context: context.clone(),
            ranked_opportunities: prioritized_research_candidates(
                &context,
                &initial_ranked,
                self.budget.max_deep_research_candidates,
            ),
            budget: self.budget.clone(),
        };
        let planned = match self.planner.plan(&planning_context) {
            Ok(requests) => requests,
            Err(error) => {
                return Ok(no_action(
                    format!("research planning failed: {error:?}"),
                    initial_ranked,
                    Vec::new(),
                    Vec::new(),
                ));
            }
        };

        let mut requests = Vec::new();
        let mut request_keys = BTreeSet::new();
        for request in planned {
            if requests.len() >= self.budget.max_source_calls {
                return Ok(no_action(
                    "research budget exhausted before source calls".to_owned(),
                    initial_ranked,
                    requests,
                    Vec::new(),
                ));
            }
            if !context.universe.contains(&request.instrument) {
                return Ok(no_action(
                    format!(
                        "research requested outside configured universe: {}",
                        request.instrument
                    ),
                    initial_ranked,
                    requests,
                    Vec::new(),
                ));
            }
            if request.rationale.trim().is_empty()
                || request.query.max_results() > self.budget.max_results_per_query
                || request.query.max_per_source() > self.budget.max_results_per_source
                || !request.query.targets_instrument(&request.instrument)
            {
                return Ok(no_action(
                    "research request exceeds its explicit budget".to_owned(),
                    initial_ranked,
                    requests,
                    Vec::new(),
                ));
            }
            let key = (request.source, request.instrument.clone());
            if request_keys.insert(key) {
                requests.push(request);
            }
        }

        observer
            .on_research_planned(&requests, context.as_of)
            .map_err(|error| DecisionError::Observer(error.to_string()))?;

        let mut effective_context = context;
        let mut research_errors = Vec::new();
        let mut known_events = effective_context
            .evidence
            .iter()
            .map(|evidence| {
                (
                    evidence.source,
                    evidence.source_event_id.as_str().to_owned(),
                )
            })
            .collect::<BTreeSet<_>>();
        for request in &requests {
            match self.researcher.research(request) {
                Ok(result) if result.source == request.source => {
                    observer
                        .on_source_completed(
                            result.source,
                            &result.evidence,
                            effective_context.as_of,
                        )
                        .map_err(|error| DecisionError::Observer(error.to_string()))?;
                    effective_context
                        .source_health
                        .record_success(result.source, effective_context.as_of);
                    for evidence in result.evidence {
                        evidence
                            .validate()
                            .map_err(|error| DecisionError::InvalidContext(error.to_string()))?;
                        if request.query.matches(&evidence)
                            && evidence
                                .affected_assets
                                .iter()
                                .any(|impact| impact.instrument == request.instrument)
                            && known_events.insert((
                                evidence.source,
                                evidence.source_event_id.as_str().to_owned(),
                            ))
                        {
                            effective_context.evidence.push(evidence);
                        }
                    }
                }
                Ok(_) => {
                    return Ok(no_action(
                        "research provider returned the wrong source".to_owned(),
                        initial_ranked,
                        requests,
                        research_errors,
                    ));
                }
                Err(error) => {
                    observer
                        .on_source_failed(&error, effective_context.as_of)
                        .map_err(|error| DecisionError::Observer(error.to_string()))?;
                    effective_context.source_health.record_failure(
                        error.source(),
                        effective_context.as_of,
                        error.category(),
                    );
                    research_errors.push(error);
                }
            }
        }

        let ranked = rank_context(&effective_context)?;
        let agent_request = AgentDecisionRequest {
            context: effective_context.clone(),
            ranked_opportunities: ranked.clone(),
            research_requests: requests.clone(),
            research_errors: research_errors.clone(),
            budget: self.budget.clone(),
        };
        observer
            .on_agent_decision_requested(effective_context.as_of)
            .map_err(|error| DecisionError::Observer(error.to_string()))?;
        let payloads = match self.decision_provider.decide(&agent_request) {
            Ok(payloads) => payloads,
            Err(error) => {
                return Ok(no_action(
                    format!("Hermes decision request failed: {error:?}"),
                    ranked,
                    requests,
                    research_errors,
                ));
            }
        };
        if payloads.is_empty() || payloads.len() > effective_context.universe.instruments().len() {
            return Ok(no_action(
                "Hermes returned no decisions or too many decisions".to_owned(),
                ranked,
                requests,
                research_errors,
            ));
        }

        let mut seen_instruments = BTreeSet::new();
        let mut seen_decision_ids = BTreeSet::new();
        let mut proposals = Vec::new();
        for payload in payloads {
            let validated = match decode_agent_decision(&payload, &effective_context.universe) {
                Ok(decision) => decision,
                Err(error) => {
                    return Ok(no_action(
                        format!("invalid Hermes decision: {error}"),
                        ranked,
                        requests,
                        research_errors,
                    ));
                }
            };
            let decision = validated.decision();
            let all_selected_sources_unavailable = !requests.is_empty()
                && research_errors.len() == requests.len()
                && effective_context.evidence.is_empty();
            if all_selected_sources_unavailable && decision.action != TradeAction::Hold {
                return Ok(no_action(
                    "all selected intelligence sources are unavailable".to_owned(),
                    ranked,
                    requests,
                    research_errors,
                ));
            }
            if !seen_instruments.insert(decision.instrument.clone())
                || !seen_decision_ids.insert(decision.decision_id)
            {
                return Ok(no_action(
                    "Hermes returned duplicate decision identity".to_owned(),
                    ranked,
                    requests,
                    research_errors,
                ));
            }
            if let Some(position) = effective_context
                .positions
                .for_instrument(&decision.instrument)
            {
                if let Err(error) = decision.validate_for_position(position) {
                    return Ok(no_action(
                        format!("decision conflicts with known position: {error}"),
                        ranked,
                        requests,
                        research_errors,
                    ));
                }
            }
            if decision.action != TradeAction::Hold
                && decision
                    .evidence_refs
                    .iter()
                    .any(|reference| reference.source == SourceKind::Reddit)
                && !decision
                    .evidence_refs
                    .iter()
                    .any(|reference| reference.source != SourceKind::Reddit)
            {
                return Ok(no_action(
                    "Reddit evidence requires non-Reddit corroboration".to_owned(),
                    ranked,
                    requests,
                    research_errors,
                ));
            }
            if effective_context
                .prior_decisions
                .iter()
                .any(|prior| prior.matches(&validated))
            {
                continue;
            }
            proposals.push(DecisionProposal {
                kind: proposal_kind(
                    decision,
                    effective_context
                        .positions
                        .for_instrument(&decision.instrument),
                ),
                decision: validated,
            });
        }

        let actionable = proposals
            .iter()
            .filter(|proposal| proposal.kind != ProposalKind::Hold)
            .count();
        if actionable > self.budget.max_actionable_decisions {
            return Ok(no_action(
                "actionable decision budget exceeded".to_owned(),
                ranked,
                requests,
                research_errors,
            ));
        }
        if proposals.is_empty() {
            return Ok(no_action(
                "unchanged decision already processed or no valid proposal remained".to_owned(),
                ranked,
                requests,
                research_errors,
            ));
        }
        Ok(DecisionCycleResult::Completed {
            ranked_opportunities: ranked,
            proposals,
            research_requests: requests,
            research_errors,
        })
    }
}

fn no_action(
    reason: String,
    ranked_opportunities: Vec<RankedOpportunityContext>,
    research_requests: Vec<ResearchRequest>,
    research_errors: Vec<SourceError>,
) -> DecisionCycleResult {
    DecisionCycleResult::NoAction {
        reason,
        ranked_opportunities,
        research_requests,
        research_errors,
    }
}

fn proposal_kind(decision: &TradeDecision, position: Option<&PositionState>) -> ProposalKind {
    match decision.action {
        TradeAction::Buy if position.is_some_and(PositionState::is_open) => ProposalKind::Add,
        TradeAction::Buy => ProposalKind::Entry,
        TradeAction::Sell => ProposalKind::Reduction,
        TradeAction::Hold => ProposalKind::Hold,
    }
}

fn rank_context(context: &DecisionContext) -> Result<Vec<RankedOpportunityContext>, DecisionError> {
    let mut candidates = Vec::with_capacity(context.universe.instruments().len());
    let mut evidence_by_instrument = BTreeMap::<Instrument, Vec<IntelligenceEvidence>>::new();
    for instrument in context.universe.instruments() {
        evidence_by_instrument.insert(instrument.clone(), Vec::new());
    }
    for evidence in &context.evidence {
        for impact in &evidence.affected_assets {
            if let Some(items) = evidence_by_instrument.get_mut(&impact.instrument) {
                items.push(evidence.clone());
            }
        }
    }
    for (instrument, evidence) in &evidence_by_instrument {
        candidates.push(crate::domain::OpportunityCandidate::new(
            instrument.clone(),
            attention_score(evidence, instrument),
        )?);
    }
    let ranked = rank_opportunities(candidates)?;
    let prior_evidence_by_instrument = context.prior_decisions.iter().fold(
        BTreeMap::<Instrument, BTreeSet<EvidenceId>>::new(),
        |mut map, prior| {
            map.entry(prior.instrument.clone())
                .or_default()
                .extend(prior.evidence_ids.iter().copied());
            map
        },
    );
    Ok(ranked
        .into_iter()
        .map(|ranking| {
            let evidence = evidence_by_instrument
                .get(&ranking.instrument)
                .cloned()
                .unwrap_or_default();
            let mut supporting_evidence = Vec::new();
            let mut counter_evidence = Vec::new();
            let mut other_evidence = Vec::new();
            let mut event_lineages = BTreeSet::new();
            for item in evidence {
                event_lineages.insert(item.event_fingerprint.clone());
                match item.direction {
                    IntelligenceDirection::Bullish => supporting_evidence.push(item),
                    IntelligenceDirection::Bearish => counter_evidence.push(item),
                    IntelligenceDirection::Neutral
                    | IntelligenceDirection::Mixed
                    | IntelligenceDirection::Unclear => other_evidence.push(item),
                }
            }
            let evidence_ids: BTreeSet<EvidenceId> = supporting_evidence
                .iter()
                .chain(counter_evidence.iter())
                .chain(other_evidence.iter())
                .map(|item| item.evidence_id)
                .collect();
            let new_evidence_ids = evidence_ids
                .difference(
                    prior_evidence_by_instrument
                        .get(&ranking.instrument)
                        .unwrap_or(&BTreeSet::new()),
                )
                .copied()
                .collect();
            RankedOpportunityContext {
                ranking,
                supporting_evidence,
                counter_evidence,
                other_evidence,
                event_lineages,
                new_evidence_ids,
            }
        })
        .collect())
}

fn prioritized_research_candidates(
    context: &DecisionContext,
    ranked: &[RankedOpportunityContext],
    limit: usize,
) -> Vec<RankedOpportunityContext> {
    let mut prioritized = Vec::with_capacity(ranked.len());
    let mut seen = BTreeSet::new();
    let mut add_instrument = |instrument: &Instrument| {
        if seen.insert(instrument.clone()) {
            if let Some(candidate) = ranked
                .iter()
                .find(|candidate| &candidate.ranking.instrument == instrument)
            {
                prioritized.push(candidate.clone());
            }
        }
    };
    match &context.trigger {
        DecisionTrigger::EventTriggered { instruments, .. } => {
            for instrument in instruments {
                add_instrument(instrument);
            }
        }
        DecisionTrigger::PositionReview { instrument, .. } => add_instrument(instrument),
        DecisionTrigger::OpportunityScan { .. } => {}
    }
    for candidate in ranked {
        if seen.insert(candidate.ranking.instrument.clone()) {
            prioritized.push(candidate.clone());
        }
    }
    prioritized.truncate(limit);
    prioritized
}

fn attention_score(evidence: &[IntelligenceEvidence], instrument: &Instrument) -> Decimal {
    let mut per_event = BTreeMap::<EventFingerprint, Decimal>::new();
    for item in evidence {
        let (Some(strength), Some(freshness), Some(reliability), Some(novelty)) = (
            item.strength,
            item.freshness,
            item.reliability,
            item.novelty,
        ) else {
            continue;
        };
        let relevance = item
            .affected_assets
            .iter()
            .find(|impact| &impact.instrument == instrument)
            .map_or(Decimal::ZERO, |impact| impact.relevance.value());
        let value = strength.value()
            * freshness.value()
            * reliability.value()
            * novelty.value()
            * relevance;
        per_event
            .entry(item.event_fingerprint.clone())
            .and_modify(|current| *current = (*current).max(value))
            .or_insert(value);
    }
    per_event
        .values()
        .copied()
        .fold(Decimal::ZERO, |total, value| {
            (total + value).min(Decimal::ONE)
        })
}
