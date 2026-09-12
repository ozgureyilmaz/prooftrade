use prooftrade::decision::{
    AgentDecisionRequest, DecisionContext, DecisionOrchestrator, DecisionProvider, DecisionTrigger,
    PositionSnapshot, ResearchPlanningContext, ResearchProvider, ResearchRequest, ResearchResult,
    ResearchSourcePlanner,
};
use prooftrade::domain::TradingUniverse;
use prooftrade::intelligence::sources::SourceError;
use prooftrade::intelligence::{
    IntelligenceEvidence, IntelligenceSource, SourceErrorCategory, SourceHealthRegistry,
};
use prooftrade::operator::{
    OperatorController, OperatorProjector, OperatorReadModel, OperatorRunPhase, OperatorRunStatus,
    render_dashboard, run_decision_cycle,
};
use prooftrade::persistence::{AuditStore, RunId};
use prooftrade::risk::{OperatingMode, SafetyState};
use rust_decimal::Decimal;
use serde_json::json;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;

#[test]
fn read_model_and_dashboard_make_unknown_execution_visible() {
    let model = OperatorReadModel {
        environment: OperatingMode::Demo,
        safety_state: SafetyState::Safe,
        execution_warning: Some("Execution state unknown. Reconciliation in progress.".to_owned()),
        ..Default::default()
    };
    let body = render_dashboard(&model);
    assert!(body.contains("DEMO"));
    assert!(body.contains("Execution state unknown"));
    assert!(body.contains("See the proof trail."));
    assert!(!body.contains("window.__PROOFTRADE_INITIAL__ = null;"));
    assert!(body.contains("\"agent\""));
    assert!(body.contains("\"signals\":[]"));
}

#[test]
fn kill_switch_requires_explicit_confirmation_and_changes_runtime_state() {
    let controller = OperatorController::new(SafetyState::Normal);
    assert!(controller.kill_switch("no").is_err());
    assert_eq!(controller.kill_switch("KILL").unwrap(), SafetyState::Killed);
    assert_eq!(controller.state(), SafetyState::Killed);
}

#[test]
fn operator_routes_expose_reads_and_safety_but_not_raw_trading() {
    let controller = OperatorController::new(SafetyState::Normal);
    let model = OperatorReadModel::default();
    let response = controller.handle("GET", "/api/read-model", "", &model);
    assert_eq!(response.status, 200);
    assert!(response.body.contains("SIMULATION"));
    let forbidden = controller.handle("POST", "/api/raw-order", "", &model);
    assert_eq!(forbidden.status, 404);
}

#[test]
fn read_model_reflects_controller_kill_state_after_confirmation() {
    let controller = OperatorController::new(SafetyState::Normal);
    let model = OperatorReadModel::default();
    let response = controller.handle("POST", "/api/safety/kill", "KILL", &model);
    assert_eq!(response.status, 200);

    let read_model = controller.handle("GET", "/api/read-model", "", &model);
    assert!(read_model.body.contains("\"safety_state\":\"KILLED\""));
}

#[test]
fn local_operator_server_serves_the_dashboard_read_model() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let controller = OperatorController::new(SafetyState::Normal);
    let model = OperatorReadModel::default();
    let handle = thread::spawn(move || controller.serve_once(listener, &model));
    let mut client = TcpStream::connect(address).unwrap();
    client
        .write_all(b"GET /api/read-model HTTP/1.1\r\nHost: localhost\r\n\r\n")
        .unwrap();
    let mut response = String::new();
    client.read_to_string(&mut response).unwrap();
    handle.join().unwrap().unwrap();
    assert!(response.contains("200 OK"));
    assert!(response.contains("SIMULATION"));
}

#[test]
fn operator_projection_updates_during_a_run_and_rehydrates_after_restart() {
    let path = std::env::temp_dir().join(format!(
        "prooftrade-operator-{}.sqlite",
        uuid::Uuid::new_v4()
    ));
    let run_id = RunId::new();
    let projector = OperatorProjector::new(
        AuditStore::open(&path).unwrap(),
        OperatorReadModel::default(),
    );

    projector
        .run_started(
            run_id,
            "cycle-1",
            OperatingMode::Simulation,
            SafetyState::Normal,
            time::OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap(),
        )
        .unwrap();
    let live = projector.snapshot();
    assert_eq!(live.revision, 1);
    assert_eq!(live.run.status, OperatorRunStatus::Running);
    assert_eq!(live.run.phase, OperatorRunPhase::Researching);

    drop(projector);
    let restored = OperatorProjector::new(
        AuditStore::open(&path).unwrap(),
        OperatorReadModel::default(),
    );
    assert!(restored.rehydrate().unwrap());
    assert_eq!(
        restored.snapshot().run.run_id,
        Some(run_id.as_uuid().to_string())
    );
    assert_eq!(restored.snapshot().revision, 1);
    drop(restored);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn source_completed_persists_evidence_in_the_runtime_audit_store() {
    let path = std::env::temp_dir().join(format!(
        "prooftrade-reddit-evidence-{}.sqlite",
        uuid::Uuid::new_v4()
    ));
    let run_id = RunId::new();
    let projector = OperatorProjector::new(
        AuditStore::open(&path).unwrap(),
        OperatorReadModel::default(),
    );
    let evidence = prooftrade::intelligence::IntelligenceEvidence {
        evidence_id: prooftrade::domain::EvidenceId::from_uuid(uuid::Uuid::new_v4()),
        source: IntelligenceSource::Reddit,
        source_event_id: prooftrade::domain::SourceEventId::new("reddit:BTC-USDT:post-1").unwrap(),
        event_time: Some(time::OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap()),
        observed_at: time::OffsetDateTime::from_unix_timestamp(1_700_000_001).unwrap(),
        affected_assets: vec![prooftrade::intelligence::AssetImpact {
            instrument: prooftrade::domain::Instrument::parse("BTC-USDT").unwrap(),
            relevance: prooftrade::intelligence::Score::new(Decimal::new(7, 1)).unwrap(),
        }],
        direction: prooftrade::intelligence::IntelligenceDirection::Bearish,
        strength: Some(prooftrade::intelligence::Score::new(Decimal::new(7, 1)).unwrap()),
        freshness: None,
        reliability: None,
        novelty: None,
        summary: "Bitcoin exploit event".to_owned(),
        source_reference: Some(
            prooftrade::intelligence::SourceReference::new(
                "https://www.reddit.com/r/BitcoinMarkets/comments/post-1/event/",
            )
            .unwrap(),
        ),
        event_fingerprint: prooftrade::intelligence::EventFingerprint::from_canonical_key(
            "btc-exploit",
        )
        .unwrap(),
        lineage_id: None,
        payload: prooftrade::intelligence::EvidencePayload::Reddit(
            prooftrade::intelligence::RedditEvidence {
                post_id: "post-1".to_owned(),
                communities: vec!["BitcoinMarkets".to_owned()],
                event_type: Some("EXPLOIT".to_owned()),
                supporting_signals: vec![],
                counter_signals: vec!["event:exploit".to_owned()],
                context_comments: Some(2),
                event_relevance: prooftrade::intelligence::Score::new(Decimal::new(7, 1)).unwrap(),
                mention_velocity: None,
                unique_author_velocity: None,
                engagement_velocity: None,
                sentiment: None,
                sentiment_delta: None,
                sentiment_dispersion: None,
                cross_community_confirmation: None,
            },
        ),
    };
    let at = evidence.observed_at;
    projector
        .run_started(
            run_id,
            "reddit-evidence",
            OperatingMode::Simulation,
            SafetyState::Normal,
            at,
        )
        .unwrap();
    projector
        .source_completed(run_id, IntelligenceSource::Reddit, &[evidence], at)
        .unwrap();
    drop(projector);

    let store = AuditStore::open(&path).unwrap();
    assert_eq!(store.count_evidence().unwrap(), 1);
    drop(store);
    std::fs::remove_file(path).unwrap();
}

struct EmptyPlanner;

impl ResearchSourcePlanner for EmptyPlanner {
    type Error = String;

    fn plan(
        &mut self,
        _context: &ResearchPlanningContext,
    ) -> Result<Vec<ResearchRequest>, Self::Error> {
        Ok(Vec::new())
    }
}

struct EmptyResearcher;

impl ResearchProvider for EmptyResearcher {
    fn research(&mut self, request: &ResearchRequest) -> Result<ResearchResult, SourceError> {
        Ok(ResearchResult::new(request.source, Vec::new()))
    }
}

struct HoldAgent;

impl DecisionProvider for HoldAgent {
    type Error = String;

    fn decide(&mut self, _request: &AgentDecisionRequest) -> Result<Vec<String>, Self::Error> {
        Ok(vec![
            json!({
                "schema_version": 1,
                "profile": "prooftrade-agent",
                "request_id": "operator-request",
                "run_id": "operator-run",
                "emitted_at": "2026-09-12T08:00:00Z",
                "decision_id": "11111111-1111-4111-8111-111111111111",
                "instrument": "BTC-USDT",
                "action": "HOLD",
                "confidence": 0.4,
                "support_strength": 0.1,
                "counter_signal_strength": 0.2,
                "expected_horizon_secs": null,
                "desired_exposure_pct": null,
                "requested_notional": null,
                "desired_reduction_pct": null,
                "urgency": "NORMAL",
                "max_acceptable_price": null,
                "why_trade": [],
                "why_not_trade": ["insufficient evidence"],
                "reconsider_if": [],
                "invalidation": null,
                "evidence_refs": [],
                "created_at": "2026-09-12T08:00:00Z"
            })
            .to_string(),
        ])
    }
}

#[test]
fn decision_cycle_projects_agent_provenance_and_source_status() {
    let path =
        std::env::temp_dir().join(format!("prooftrade-cycle-{}.sqlite", uuid::Uuid::new_v4()));
    let run_id = RunId::new();
    let projector = OperatorProjector::new(
        AuditStore::open(&path).unwrap(),
        OperatorReadModel::default(),
    );
    let mut orchestrator =
        DecisionOrchestrator::new(EmptyPlanner, EmptyResearcher, HoldAgent, Default::default());
    let context = DecisionContext::new(
        DecisionTrigger::OpportunityScan {
            reason: "operator test".to_owned(),
        },
        TradingUniverse::initial(),
        time::OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap(),
        Vec::<IntelligenceEvidence>::new(),
        Vec::new(),
        PositionSnapshot::new(
            time::OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap(),
            Vec::new(),
        )
        .unwrap(),
        SourceHealthRegistry::new(),
    )
    .unwrap();

    let cycle = run_decision_cycle(
        &mut orchestrator,
        context,
        run_id,
        "cycle-1",
        OperatingMode::Simulation,
        SafetyState::Normal,
        &projector,
    )
    .unwrap();

    assert_eq!(cycle.proposals().len(), 1);
    let model = projector.snapshot();
    assert_eq!(model.run.status, OperatorRunStatus::Running);
    assert_eq!(model.run.phase, OperatorRunPhase::RiskCheck);
    assert_eq!(model.latest_proposal_kind.as_deref(), Some("hold"));
    assert_eq!(model.agent.profile, "prooftrade-agent");
    assert_eq!(
        model.decision_provenance.as_ref().unwrap().request_id,
        "operator-request"
    );

    projector
        .source_failed(
            run_id,
            &SourceError::new(IntelligenceSource::Reddit, SourceErrorCategory::Timeout),
            time::OffsetDateTime::from_unix_timestamp(1_700_000_001).unwrap(),
        )
        .unwrap();
    let model = projector.snapshot();
    let reddit = model
        .sources
        .iter()
        .find(|source| source.source == IntelligenceSource::Reddit)
        .unwrap();
    assert_eq!(
        reddit.status,
        prooftrade::intelligence::SourceHealthStatus::Degraded
    );

    drop(projector);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn persistent_operator_controller_reads_updates_from_the_projection_store() {
    let path =
        std::env::temp_dir().join(format!("prooftrade-host-{}.sqlite", uuid::Uuid::new_v4()));
    let run_id = RunId::new();
    let projector = OperatorProjector::new(
        AuditStore::open(&path).unwrap(),
        OperatorReadModel::default(),
    );
    projector
        .run_started(
            run_id,
            "cycle-1",
            OperatingMode::Simulation,
            SafetyState::Normal,
            time::OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap(),
        )
        .unwrap();
    drop(projector);

    let controller = OperatorController::with_persistent_store(
        OperatorReadModel::default(),
        prooftrade::risk::SafetyStateStore::new(SafetyState::Normal),
        AuditStore::open(&path).unwrap(),
    );
    let response = controller.handle_current("GET", "/api/read-model", "");
    assert_eq!(response.status, 200);
    assert!(response.body.contains("\"revision\":1"));
    assert!(response.body.contains(&run_id.as_uuid().to_string()));

    drop(controller);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn projected_operator_kill_is_persisted_with_the_shared_safety_state() {
    let path =
        std::env::temp_dir().join(format!("prooftrade-kill-{}.sqlite", uuid::Uuid::new_v4()));
    let run_id = RunId::new();
    let projector = OperatorProjector::new(
        AuditStore::open(&path).unwrap(),
        OperatorReadModel::default(),
    );
    projector
        .run_started(
            run_id,
            "cycle-1",
            OperatingMode::Simulation,
            SafetyState::Normal,
            time::OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap(),
        )
        .unwrap();
    let controller = OperatorController::with_projector(
        projector,
        prooftrade::risk::SafetyStateStore::new(SafetyState::Normal),
    );

    controller.kill_switch("KILL").unwrap();
    let response = controller.handle_current("GET", "/api/read-model", "");
    assert!(response.body.contains("\"safety_state\":\"KILLED\""));

    drop(controller);
    std::fs::remove_file(path).unwrap();
}
