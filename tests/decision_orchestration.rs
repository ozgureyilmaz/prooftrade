use std::sync::{Arc, Mutex};

use prooftrade::decision::{
    AgentDecisionRequest, DecisionContext, DecisionCycleResult, DecisionOrchestrator,
    DecisionProvider, DecisionTrigger, MarketContext, PositionSnapshot, PriorDecision,
    ProposalKind, ResearchBudget, ResearchPlanningContext, ResearchProvider, ResearchRequest,
    ResearchResult, ResearchSourcePlanner,
};
use prooftrade::domain::{
    DecisionId, EvidenceId, Instrument, LineageId, PositionId, PositionState, SourceEventId,
    TradeAction, TradingUniverse,
};
use prooftrade::intelligence::sources::SourceError;
use prooftrade::intelligence::{
    AssetImpact, EventFingerprint, EvidencePayload, IntelligenceDirection, IntelligenceEvidence,
    IntelligenceSource, MarxEvidence, NewsEvidence, PredictionMarketEvidence, Probability,
    RedditEvidence, Score, SourceHealthRegistry, SourceReference,
};
use prooftrade::intelligence::{SourceErrorCategory, SourceHealthStatus};
use rust_decimal::Decimal;
use serde_json::json;
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Clone)]
struct FakePlanner {
    requests: Vec<ResearchRequest>,
    calls: Arc<Mutex<usize>>,
}

impl ResearchSourcePlanner for FakePlanner {
    type Error = String;

    fn plan(
        &mut self,
        _context: &ResearchPlanningContext,
    ) -> Result<Vec<ResearchRequest>, Self::Error> {
        *self.calls.lock().unwrap() += 1;
        Ok(self.requests.clone())
    }
}

struct FakeResearcher {
    calls: Arc<Mutex<Vec<ResearchRequest>>>,
    result: Result<ResearchResult, prooftrade::intelligence::sources::SourceError>,
}

impl ResearchProvider for FakeResearcher {
    fn research(
        &mut self,
        request: &ResearchRequest,
    ) -> Result<ResearchResult, prooftrade::intelligence::sources::SourceError> {
        self.calls.lock().unwrap().push(request.clone());
        self.result.clone()
    }
}

#[derive(Clone)]
struct FakeAgent {
    payloads: Vec<String>,
    calls: Arc<Mutex<Vec<AgentDecisionRequest>>>,
    error: Option<String>,
}

impl DecisionProvider for FakeAgent {
    type Error = String;

    fn decide(&mut self, request: &AgentDecisionRequest) -> Result<Vec<String>, Self::Error> {
        self.calls.lock().unwrap().push(request.clone());
        self.error
            .clone()
            .map_or_else(|| Ok(self.payloads.clone()), Err)
    }
}

fn now() -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap()
}

fn instrument(symbol: &str) -> Instrument {
    Instrument::parse(symbol).unwrap()
}

fn universe() -> TradingUniverse {
    TradingUniverse::initial()
}

fn position_snapshot(positions: Vec<PositionState>) -> PositionSnapshot {
    PositionSnapshot::new(now(), positions).unwrap()
}

fn context(evidence: Vec<IntelligenceEvidence>, positions: Vec<PositionState>) -> DecisionContext {
    DecisionContext::new(
        DecisionTrigger::OpportunityScan {
            reason: "event scan".to_owned(),
        },
        universe(),
        now(),
        evidence,
        vec![],
        position_snapshot(positions),
        SourceHealthRegistry::new(),
    )
    .unwrap()
}

fn context_with_trigger(
    trigger: DecisionTrigger,
    evidence: Vec<IntelligenceEvidence>,
    positions: Vec<PositionState>,
) -> DecisionContext {
    DecisionContext::new(
        trigger,
        universe(),
        now(),
        evidence,
        vec![],
        position_snapshot(positions),
        SourceHealthRegistry::new(),
    )
    .unwrap()
}

fn market(instrument: Instrument, last: i64) -> MarketContext {
    MarketContext::new(
        instrument,
        now(),
        Decimal::from(last - 1),
        Decimal::from(last + 1),
        Decimal::from(last),
    )
    .unwrap()
}

fn evidence(
    source: IntelligenceSource,
    direction: IntelligenceDirection,
    evidence_number: u128,
    lineage_number: u128,
    asset: &str,
    strength: Decimal,
) -> IntelligenceEvidence {
    let asset = instrument(asset);
    let source_event_id = SourceEventId::new(format!("event-{evidence_number}")).unwrap();
    let event_fingerprint =
        EventFingerprint::from_canonical_key(format!("event-{lineage_number}")).unwrap();
    let payload = match source {
        IntelligenceSource::AtkNews => EvidencePayload::News(NewsEvidence {
            headline: "event headline".to_owned(),
            summary: Some("event summary".to_owned()),
            category: Some("market".to_owned()),
            importance: Some(Score::new(strength).unwrap()),
            published_at: Some(now()),
        }),
        IntelligenceSource::Polymarket => {
            EvidencePayload::PredictionMarket(PredictionMarketEvidence {
                market_id: format!("market-{evidence_number}"),
                token_id: format!("token-{evidence_number}"),
                question: "event question".to_owned(),
                implied_probability: Probability::new(Decimal::new(60, 2)).unwrap(),
                probability_delta: None,
                liquidity: Some(Decimal::from(1_000)),
                volume: Some(Decimal::from(2_000)),
                spread: Some(Decimal::new(1, 2)),
            })
        }
        IntelligenceSource::Reddit => EvidencePayload::Reddit(RedditEvidence {
            post_id: format!("post-{evidence_number}"),
            communities: vec!["CryptoCurrency".to_owned()],
            event_type: Some("MARKET_MOVE".to_owned()),
            supporting_signals: vec![],
            counter_signals: vec![],
            context_comments: None,
            event_relevance: Score::new(strength).unwrap(),
            mention_velocity: Some(Decimal::from(10)),
            unique_author_velocity: None,
            engagement_velocity: None,
            sentiment: None,
            sentiment_delta: None,
            sentiment_dispersion: None,
            cross_community_confirmation: None,
        }),
        IntelligenceSource::MarxFinance => EvidencePayload::Marx(MarxEvidence {
            agent_id: format!("agent-{evidence_number}"),
            thesis: "event thesis".to_owned(),
            stance: direction,
            supporting_arguments: vec!["support".to_owned()],
            counter_arguments: vec!["counter".to_owned()],
            confidence: Some(Score::new(strength).unwrap()),
        }),
    };
    IntelligenceEvidence {
        evidence_id: EvidenceId::from_uuid(Uuid::from_u128(evidence_number)),
        source,
        source_event_id,
        event_time: Some(now()),
        observed_at: now(),
        affected_assets: vec![AssetImpact {
            instrument: asset,
            relevance: Score::new(Decimal::new(95, 2)).unwrap(),
        }],
        direction,
        strength: Some(Score::new(strength).unwrap()),
        freshness: Some(Score::new(Decimal::new(90, 2)).unwrap()),
        reliability: Some(Score::new(Decimal::new(85, 2)).unwrap()),
        novelty: Some(Score::new(Decimal::new(80, 2)).unwrap()),
        summary: "event summary".to_owned(),
        source_reference: Some(
            SourceReference::new(format!("source://{evidence_number}")).unwrap(),
        ),
        event_fingerprint,
        lineage_id: Some(LineageId::from_uuid(Uuid::from_u128(lineage_number))),
        payload,
    }
}

fn decision_payload(
    instrument: &str,
    action: &str,
    evidence: Option<&IntelligenceEvidence>,
) -> String {
    let mut payload = json!({
        "schema_version": 1,
        "profile": "prooftrade-agent",
        "request_id": "request-1",
        "run_id": "run-1",
        "emitted_at": "2026-09-12T08:00:00Z",
        "decision_id": "11111111-1111-4111-8111-111111111111",
        "instrument": instrument,
        "action": action,
        "confidence": 0.84,
        "support_strength": 0.88,
        "counter_signal_strength": 0.22,
        "expected_horizon_secs": 900,
        "urgency": "HIGH",
        "why_trade": ["fresh event"],
        "why_not_trade": ["counter-signal is bounded"],
        "reconsider_if": ["volume changes"],
        "invalidation": {"description": "event loses confirmation"},
        "evidence_refs": [],
        "created_at": "2026-09-12T08:00:00Z"
    });
    match action {
        "BUY" => {
            payload["desired_exposure_pct"] = json!(0.18);
            payload["requested_notional"] = json!("150.00");
            payload["max_acceptable_price"] = json!("60010.00");
        }
        "SELL" => {
            payload["desired_reduction_pct"] = json!(0.40);
        }
        "HOLD" => {
            payload["why_not_trade"] = json!(["no new confirmation"]);
            payload["why_trade"] = json!([]);
        }
        _ => {}
    }
    if let Some(evidence) = evidence {
        payload["evidence_refs"] = json!([{
            "evidence_id": evidence.evidence_id.as_uuid(),
            "source": serde_json::to_value(evidence.source).unwrap(),
            "source_event_id": evidence.source_event_id.as_str(),
            "lineage_id": evidence.lineage_id.unwrap().as_uuid(),
            "instrument": instrument,
            "observed_at": "2026-09-12T08:00:00Z",
            "direction": if evidence.direction == IntelligenceDirection::Bearish { "counter" } else { "supporting" },
            "strength": evidence.strength.unwrap().value(),
            "freshness": evidence.freshness.unwrap().value(),
            "reliability": evidence.reliability.unwrap().value(),
            "novelty": evidence.novelty.unwrap().value(),
            "relevance": 0.95,
            "source_reference": evidence.source_reference.as_ref().unwrap().as_str()
        }]);
    }
    serde_json::to_string(&payload).unwrap()
}

fn orchestrator(
    planner_requests: Vec<ResearchRequest>,
    research_calls: Arc<Mutex<Vec<ResearchRequest>>>,
    agent_payloads: Vec<String>,
    agent_calls: Arc<Mutex<Vec<AgentDecisionRequest>>>,
) -> DecisionOrchestrator<FakePlanner, FakeResearcher, FakeAgent> {
    DecisionOrchestrator::new(
        FakePlanner {
            requests: planner_requests,
            calls: Arc::new(Mutex::new(0)),
        },
        FakeResearcher {
            calls: research_calls,
            result: Ok(ResearchResult::new(IntelligenceSource::MarxFinance, vec![])),
        },
        FakeAgent {
            payloads: agent_payloads,
            calls: agent_calls,
            error: None,
        },
        ResearchBudget::default(),
    )
}

fn research_request(source: IntelligenceSource, symbol: &str) -> ResearchRequest {
    let query = prooftrade::intelligence::BoundedEvidenceQuery::new(now(), 5, 3)
        .unwrap()
        .with_instruments([instrument(symbol)]);
    ResearchRequest::new(source, instrument(symbol), query, "event relevance")
}

fn hold_with_id(symbol: &str, decision_id: &str) -> String {
    decision_payload(symbol, "HOLD", None)
        .replace("11111111-1111-4111-8111-111111111111", decision_id)
}

#[test]
fn configured_universe_is_ranked_deterministically_before_agent_decision() {
    let strong = evidence(
        IntelligenceSource::MarxFinance,
        IntelligenceDirection::Bullish,
        10,
        20,
        "SOL-USDT",
        Decimal::new(95, 2),
    );
    let weak = evidence(
        IntelligenceSource::Reddit,
        IntelligenceDirection::Bullish,
        11,
        21,
        "BTC-USDT",
        Decimal::new(20, 2),
    );
    let research_calls = Arc::new(Mutex::new(Vec::new()));
    let agent_calls = Arc::new(Mutex::new(Vec::new()));
    let mut engine = orchestrator(
        vec![],
        research_calls,
        vec![decision_payload("SOL-USDT", "HOLD", None)],
        agent_calls,
    );

    let result = engine.run(context(vec![strong, weak], vec![])).unwrap();
    let ranked = result.ranked_opportunities();

    assert_eq!(ranked.len(), 4);
    assert_eq!(ranked[0].ranking.instrument, instrument("SOL-USDT"));
    assert_eq!(ranked[0].ranking.rank, 1);
    assert!(ranked[0].ranking.opportunity_score > ranked[1].ranking.opportunity_score);
}

#[test]
fn reddit_only_non_hold_proposal_is_rejected_without_non_reddit_corroboration() {
    let reddit = evidence(
        IntelligenceSource::Reddit,
        IntelligenceDirection::Bullish,
        120,
        121,
        "BTC-USDT",
        Decimal::new(90, 2),
    );
    let result = DecisionOrchestrator::new(
        FakePlanner {
            requests: vec![],
            calls: Arc::new(Mutex::new(0)),
        },
        FakeResearcher {
            calls: Arc::new(Mutex::new(Vec::new())),
            result: Ok(ResearchResult::new(IntelligenceSource::Reddit, vec![])),
        },
        FakeAgent {
            payloads: vec![decision_payload("BTC-USDT", "BUY", Some(&reddit))],
            calls: Arc::new(Mutex::new(Vec::new())),
            error: None,
        },
        ResearchBudget::default(),
    )
    .run(context(vec![reddit], vec![]))
    .unwrap();

    assert_eq!(
        result.no_action_reason(),
        Some("Reddit evidence requires non-Reddit corroboration")
    );
}

#[test]
fn evidence_relevance_is_scoped_to_the_matching_asset() {
    let mut multi_asset = evidence(
        IntelligenceSource::MarxFinance,
        IntelligenceDirection::Bullish,
        12,
        22,
        "BTC-USDT",
        Decimal::new(90, 2),
    );
    multi_asset.affected_assets.push(AssetImpact {
        instrument: instrument("ETH-USDT"),
        relevance: Score::new(Decimal::new(10, 2)).unwrap(),
    });
    let mut engine = orchestrator(
        vec![],
        Arc::new(Mutex::new(Vec::new())),
        vec![decision_payload("BTC-USDT", "HOLD", None)],
        Arc::new(Mutex::new(Vec::new())),
    );

    let result = engine.run(context(vec![multi_asset], vec![])).unwrap();

    assert_eq!(
        result.ranked_opportunities()[0].ranking.instrument,
        instrument("BTC-USDT")
    );
    assert!(
        result.ranked_opportunities()[0].ranking.opportunity_score
            > result.ranked_opportunities()[1].ranking.opportunity_score
    );
}

#[test]
fn supporting_and_counter_evidence_remain_separate_in_agent_context() {
    let support = evidence(
        IntelligenceSource::MarxFinance,
        IntelligenceDirection::Bullish,
        30,
        40,
        "BTC-USDT",
        Decimal::new(90, 2),
    );
    let counter = evidence(
        IntelligenceSource::MarxFinance,
        IntelligenceDirection::Bearish,
        31,
        40,
        "BTC-USDT",
        Decimal::new(70, 2),
    );
    let research_calls = Arc::new(Mutex::new(Vec::new()));
    let agent_calls = Arc::new(Mutex::new(Vec::new()));
    let mut engine = orchestrator(
        vec![],
        research_calls,
        vec![decision_payload("BTC-USDT", "HOLD", Some(&support))],
        Arc::clone(&agent_calls),
    );

    engine.run(context(vec![support, counter], vec![])).unwrap();
    let request = &agent_calls.lock().unwrap()[0];
    let btc = request
        .ranked_opportunities
        .iter()
        .find(|candidate| candidate.ranking.instrument == instrument("BTC-USDT"))
        .unwrap();

    assert_eq!(btc.supporting_evidence.len(), 1);
    assert_eq!(btc.counter_evidence.len(), 1);
    assert_eq!(
        btc.counter_evidence[0].direction,
        IntelligenceDirection::Bearish
    );
}

#[test]
fn research_queries_only_sources_selected_by_planner() {
    let request = research_request(IntelligenceSource::MarxFinance, "SOL-USDT");
    let research_calls = Arc::new(Mutex::new(Vec::new()));
    let agent_calls = Arc::new(Mutex::new(Vec::new()));
    let mut engine = orchestrator(
        vec![request],
        Arc::clone(&research_calls),
        vec![decision_payload("SOL-USDT", "HOLD", None)],
        agent_calls,
    );

    engine.run(context(vec![], vec![])).unwrap();

    let calls = research_calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].source, IntelligenceSource::MarxFinance);
    assert_eq!(calls[0].instrument, instrument("SOL-USDT"));
}

#[test]
fn research_budget_blocks_unbounded_planner_output() {
    let requests = vec![
        research_request(IntelligenceSource::MarxFinance, "BTC-USDT"),
        research_request(IntelligenceSource::MarxFinance, "ETH-USDT"),
        research_request(IntelligenceSource::MarxFinance, "SOL-USDT"),
    ];
    let research_calls = Arc::new(Mutex::new(Vec::new()));
    let agent_calls = Arc::new(Mutex::new(Vec::new()));
    let mut engine = orchestrator(requests, Arc::clone(&research_calls), vec![], agent_calls)
        .with_budget(ResearchBudget::default().with_max_source_calls(2));

    let result = engine.run(context(vec![], vec![])).unwrap();

    assert!(matches!(result, DecisionCycleResult::NoAction { .. }));
    assert!(research_calls.lock().unwrap().is_empty());
}

#[test]
fn unscoped_research_query_is_rejected_before_provider_call() {
    let query = prooftrade::intelligence::BoundedEvidenceQuery::new(now(), 5, 3).unwrap();
    let request = ResearchRequest::new(
        IntelligenceSource::MarxFinance,
        instrument("BTC-USDT"),
        query,
        "not targeted",
    );
    let research_calls = Arc::new(Mutex::new(Vec::new()));
    let mut engine = orchestrator(
        vec![request],
        Arc::clone(&research_calls),
        vec![decision_payload("BTC-USDT", "HOLD", None)],
        Arc::new(Mutex::new(Vec::new())),
    );

    let result = engine.run(context(vec![], vec![])).unwrap();

    assert!(matches!(result, DecisionCycleResult::NoAction { .. }));
    assert!(research_calls.lock().unwrap().is_empty());
}

#[test]
fn hold_is_a_successful_proposal_and_is_not_retried() {
    let agent_calls = Arc::new(Mutex::new(Vec::new()));
    let mut engine = orchestrator(
        vec![],
        Arc::new(Mutex::new(Vec::new())),
        vec![decision_payload("BTC-USDT", "HOLD", None)],
        Arc::clone(&agent_calls),
    );

    let result = engine.run(context(vec![], vec![])).unwrap();

    assert_eq!(result.proposals().len(), 1);
    assert_eq!(result.proposals()[0].kind, ProposalKind::Hold);
    assert_eq!(agent_calls.lock().unwrap().len(), 1);
}

#[test]
fn all_assets_can_return_hold_without_being_treated_as_failure() {
    let payloads = vec![
        hold_with_id("BTC-USDT", "11111111-1111-4111-8111-111111111111"),
        hold_with_id("ETH-USDT", "22222222-2222-4222-8222-222222222222"),
        hold_with_id("SOL-USDT", "33333333-3333-4333-8333-333333333333"),
        hold_with_id("HYPE-USDT", "44444444-4444-4444-8444-444444444444"),
    ];
    let mut engine = orchestrator(
        vec![],
        Arc::new(Mutex::new(Vec::new())),
        payloads,
        Arc::new(Mutex::new(Vec::new())),
    );

    let result = engine.run(context(vec![], vec![])).unwrap();

    assert_eq!(result.proposals().len(), 4);
    assert!(
        result
            .proposals()
            .iter()
            .all(|proposal| proposal.kind == ProposalKind::Hold)
    );
}

#[test]
fn source_failure_stays_visible_while_other_cycle_work_can_continue() {
    let research_calls = Arc::new(Mutex::new(Vec::new()));
    let agent_calls = Arc::new(Mutex::new(Vec::new()));
    let planner = FakePlanner {
        requests: vec![research_request(IntelligenceSource::Reddit, "BTC-USDT")],
        calls: Arc::new(Mutex::new(0)),
    };
    let researcher = FakeResearcher {
        calls: Arc::clone(&research_calls),
        result: Err(SourceError::new(
            IntelligenceSource::Reddit,
            SourceErrorCategory::Timeout,
        )),
    };
    let agent = FakeAgent {
        payloads: vec![decision_payload("BTC-USDT", "HOLD", None)],
        calls: Arc::clone(&agent_calls),
        error: None,
    };
    let mut engine =
        DecisionOrchestrator::new(planner, researcher, agent, ResearchBudget::default());

    let result = engine.run(context(vec![], vec![])).unwrap();
    let request = &agent_calls.lock().unwrap()[0];

    assert_eq!(result.proposals().len(), 1);
    assert_eq!(result.research_errors().len(), 1);
    assert_eq!(
        request
            .context
            .source_health
            .status(IntelligenceSource::Reddit),
        SourceHealthStatus::Degraded
    );
    assert_eq!(research_calls.lock().unwrap().len(), 1);
}

#[test]
fn all_selected_source_failures_block_an_actionable_proposal_without_evidence() {
    let planner = FakePlanner {
        requests: vec![research_request(IntelligenceSource::Reddit, "BTC-USDT")],
        calls: Arc::new(Mutex::new(0)),
    };
    let researcher = FakeResearcher {
        calls: Arc::new(Mutex::new(Vec::new())),
        result: Err(SourceError::new(
            IntelligenceSource::Reddit,
            SourceErrorCategory::Unavailable,
        )),
    };
    let agent = FakeAgent {
        payloads: vec![decision_payload("BTC-USDT", "BUY", None)],
        calls: Arc::new(Mutex::new(Vec::new())),
        error: None,
    };
    let mut engine =
        DecisionOrchestrator::new(planner, researcher, agent, ResearchBudget::default());

    let result = engine.run(context(vec![], vec![])).unwrap();

    assert!(matches!(result, DecisionCycleResult::NoAction { .. }));
}

#[test]
fn hermes_timeout_returns_no_action_without_a_synthetic_trade() {
    let planner = FakePlanner {
        requests: vec![],
        calls: Arc::new(Mutex::new(0)),
    };
    let researcher = FakeResearcher {
        calls: Arc::new(Mutex::new(Vec::new())),
        result: Ok(ResearchResult::new(IntelligenceSource::MarxFinance, vec![])),
    };
    let agent_calls = Arc::new(Mutex::new(Vec::new()));
    let agent = FakeAgent {
        payloads: vec![],
        calls: Arc::clone(&agent_calls),
        error: Some("timeout".to_owned()),
    };
    let mut engine =
        DecisionOrchestrator::new(planner, researcher, agent, ResearchBudget::default());

    let result = engine.run(context(vec![], vec![])).unwrap();

    assert!(matches!(result, DecisionCycleResult::NoAction { .. }));
    assert_eq!(agent_calls.lock().unwrap().len(), 1);
}

#[test]
fn buy_preserves_agent_intent_as_proposed_entry() {
    let evidence = evidence(
        IntelligenceSource::MarxFinance,
        IntelligenceDirection::Bullish,
        50,
        60,
        "SOL-USDT",
        Decimal::new(88, 2),
    );
    let mut engine = orchestrator(
        vec![],
        Arc::new(Mutex::new(Vec::new())),
        vec![decision_payload("SOL-USDT", "BUY", Some(&evidence))],
        Arc::new(Mutex::new(Vec::new())),
    );

    let result = engine.run(context(vec![evidence], vec![])).unwrap();
    let decision = result.proposals()[0].decision.decision();

    assert_eq!(decision.action, TradeAction::Buy);
    assert_eq!(decision.requested_notional, Some(Decimal::new(15_000, 2)));
    assert_eq!(result.proposals()[0].kind, ProposalKind::Entry);
}

#[test]
fn partial_sell_for_open_position_remains_a_proposed_reduction() {
    let btc = instrument("BTC-USDT");
    let position = PositionState::open(
        PositionId::from_uuid(Uuid::from_u128(70)),
        btc.clone(),
        Decimal::ONE,
        Decimal::from(60_000),
        now(),
        now(),
    )
    .unwrap();
    let mut engine = orchestrator(
        vec![],
        Arc::new(Mutex::new(Vec::new())),
        vec![decision_payload("BTC-USDT", "SELL", None)],
        Arc::new(Mutex::new(Vec::new())),
    );

    let result = engine
        .run(context_with_trigger(
            DecisionTrigger::PositionReview {
                instrument: btc,
                reason: "new counter-signal".to_owned(),
            },
            vec![],
            vec![position],
        ))
        .unwrap();

    assert_eq!(
        result.proposals()[0]
            .decision
            .decision()
            .desired_reduction_pct,
        Some(Decimal::new(40, 2))
    );
    assert_eq!(result.proposals()[0].kind, ProposalKind::Reduction);
}

#[test]
fn sell_for_known_flat_position_is_rejected_without_a_trade_proposal() {
    let btc = instrument("BTC-USDT");
    let mut engine = orchestrator(
        vec![],
        Arc::new(Mutex::new(Vec::new())),
        vec![decision_payload("BTC-USDT", "SELL", None)],
        Arc::new(Mutex::new(Vec::new())),
    );

    let result = engine
        .run(context_with_trigger(
            DecisionTrigger::PositionReview {
                instrument: btc.clone(),
                reason: "review".to_owned(),
            },
            vec![],
            vec![PositionState::flat(btc, now())],
        ))
        .unwrap();

    assert!(matches!(result, DecisionCycleResult::NoAction { .. }));
}

#[test]
fn unchanged_evidence_does_not_repeat_a_prior_buy_after_price_change() {
    let evidence = evidence(
        IntelligenceSource::MarxFinance,
        IntelligenceDirection::Bullish,
        80,
        90,
        "BTC-USDT",
        Decimal::new(85, 2),
    );
    let decision_id = DecisionId::from_uuid(Uuid::from_u128(81));
    let prior = PriorDecision::new(
        decision_id,
        instrument("BTC-USDT"),
        TradeAction::Buy,
        [evidence.evidence_id],
    );
    let mut context = context(vec![evidence.clone()], vec![]).with_prior_decisions(vec![prior]);
    context
        .set_market_context(vec![market(instrument("BTC-USDT"), 70_000)])
        .unwrap();
    let mut engine = orchestrator(
        vec![],
        Arc::new(Mutex::new(Vec::new())),
        vec![decision_payload("BTC-USDT", "BUY", Some(&evidence))],
        Arc::new(Mutex::new(Vec::new())),
    );

    let result = engine.run(context).unwrap();

    assert!(matches!(result, DecisionCycleResult::NoAction { .. }));
}

#[test]
fn new_reaction_to_existing_lineage_can_produce_an_add_proposal() {
    let old = evidence(
        IntelligenceSource::MarxFinance,
        IntelligenceDirection::Bullish,
        100,
        110,
        "SOL-USDT",
        Decimal::new(70, 2),
    );
    let mut reaction = evidence(
        IntelligenceSource::Reddit,
        IntelligenceDirection::Bullish,
        101,
        110,
        "SOL-USDT",
        Decimal::new(90, 2),
    );
    reaction.event_fingerprint = old.event_fingerprint.clone();
    let position = PositionState::open(
        PositionId::from_uuid(Uuid::from_u128(111)),
        instrument("SOL-USDT"),
        Decimal::ONE,
        Decimal::from(100),
        now(),
        now(),
    )
    .unwrap();
    let prior = PriorDecision::new(
        DecisionId::from_uuid(Uuid::from_u128(112)),
        instrument("SOL-USDT"),
        TradeAction::Buy,
        [old.evidence_id],
    );
    let context = context(vec![old.clone(), reaction.clone()], vec![position])
        .with_prior_decisions(vec![prior]);
    let mut payload: serde_json::Value =
        serde_json::from_str(&decision_payload("SOL-USDT", "BUY", Some(&reaction))).unwrap();
    payload["evidence_refs"]
        .as_array_mut()
        .unwrap()
        .push(json!({
            "evidence_id": old.evidence_id.as_uuid(),
            "source": "marx_finance",
            "source_event_id": old.source_event_id.as_str(),
            "lineage_id": old.lineage_id.unwrap().as_uuid(),
            "instrument": "SOL-USDT",
            "observed_at": "2026-09-12T08:00:00Z",
            "direction": "supporting",
            "strength": old.strength.unwrap().value(),
            "freshness": old.freshness.unwrap().value(),
            "reliability": old.reliability.unwrap().value(),
            "novelty": old.novelty.unwrap().value(),
            "relevance": 0.95,
            "source_reference": old.source_reference.as_ref().unwrap().as_str()
        }));
    let mut engine = orchestrator(
        vec![],
        Arc::new(Mutex::new(Vec::new())),
        vec![serde_json::to_string(&payload).unwrap()],
        Arc::new(Mutex::new(Vec::new())),
    );

    let result = engine.run(context).unwrap();

    assert_eq!(result.proposals()[0].kind, ProposalKind::Add);
}

#[test]
fn malformed_agent_output_becomes_no_action_without_fallback_parsing() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let mut engine = orchestrator(
        vec![],
        Arc::new(Mutex::new(Vec::new())),
        vec!["BUY BTC-USDT".to_owned()],
        Arc::clone(&calls),
    );

    let result = engine.run(context(vec![], vec![])).unwrap();

    assert!(matches!(result, DecisionCycleResult::NoAction { .. }));
    assert_eq!(calls.lock().unwrap().len(), 1);
}

#[test]
fn duplicate_instrument_decisions_are_not_emitted_together() {
    let mut first = decision_payload("BTC-USDT", "BUY", None);
    let second = decision_payload("BTC-USDT", "SELL", None);
    first = first.replace("request-1", "request-buy");
    let mut engine = orchestrator(
        vec![],
        Arc::new(Mutex::new(Vec::new())),
        vec![first, second],
        Arc::new(Mutex::new(Vec::new())),
    );

    let result = engine.run(context(vec![], vec![])).unwrap();

    assert!(matches!(result, DecisionCycleResult::NoAction { .. }));
}
