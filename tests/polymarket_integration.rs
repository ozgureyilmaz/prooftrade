use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use prooftrade::atk::{McpError, McpTransport};
use prooftrade::decision::{
    AgentDecisionRequest, DecisionOrchestrator, DecisionProvider, ResearchPlanningContext,
    ResearchProvider, ResearchRequest, ResearchSourcePlanner,
};
use prooftrade::domain::{Instrument, SourceEventId};
use prooftrade::intelligence::sources::polymarket::{
    PolymarketAdapter, PolymarketConfig, PolymarketHybridReader, PolymarketMcpReader,
    PolymarketReader, PolymarketRestError, PolymarketRestReader, PolymarketRestTransport,
};
use prooftrade::intelligence::{
    AssetImpact, BoundedEvidenceQuery, EventFingerprint, EvidencePayload, IntelligenceDirection,
    IntelligenceEvidence, IntelligenceService, IntelligenceSource, PredictionMarketEvidence,
    Probability, Score, SignedValue, SourceReference,
};
use rust_decimal::Decimal;
use serde_json::{Value, json};
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Clone, Default)]
struct CountingReader {
    calls: Arc<Mutex<usize>>,
}

impl PolymarketReader for CountingReader {
    fn collect_polymarket(
        &mut self,
        _query: &BoundedEvidenceQuery,
    ) -> Result<Vec<IntelligenceEvidence>, prooftrade::intelligence::sources::SourceError> {
        *self.calls.lock().unwrap() += 1;
        Ok(Vec::new())
    }
}

#[test]
fn disabled_polymarket_source_is_not_queried() {
    let calls = Arc::new(Mutex::new(0));
    let mut service = IntelligenceService::new();
    service.register(PolymarketAdapter::with_enabled(
        CountingReader {
            calls: Arc::clone(&calls),
        },
        true,
    ));
    assert!(service.disable_polymarket());

    let query = query();
    let snapshot = service.collect(&query).unwrap();

    assert_eq!(*calls.lock().unwrap(), 0);
    assert!(
        snapshot
            .disabled_sources()
            .contains(&IntelligenceSource::Polymarket)
    );
    assert_eq!(
        snapshot
            .source_health()
            .status(IntelligenceSource::Polymarket),
        prooftrade::intelligence::SourceHealthStatus::Disabled
    );
}

#[test]
fn polymarket_source_can_be_reenabled_without_rebuilding_service() {
    let calls = Arc::new(Mutex::new(0));
    let mut service = IntelligenceService::new();
    service.register(PolymarketAdapter::with_enabled(
        CountingReader {
            calls: Arc::clone(&calls),
        },
        false,
    ));

    assert!(service.enable_polymarket());
    service.collect(&query()).unwrap();

    assert_eq!(*calls.lock().unwrap(), 1);
}

#[test]
fn disabled_polymarket_source_excludes_cached_evidence() {
    let mut service = IntelligenceService::new();
    service.register(PolymarketAdapter::new(FixtureReader {
        evidence: vec![sample_evidence()],
    }));
    service.collect(&query()).unwrap();

    service.disable_polymarket();
    let snapshot = service.collect(&query()).unwrap();

    assert!(snapshot.observations().is_empty());
}

#[test]
fn polymarket_mcp_reader_maps_search_result_to_prediction_evidence() {
    let transport = FakeMcpTransport::with_search_result(json!({
        "count": 1,
        "keyword": "bitcoin",
        "markets": [{
            "id": "market-1",
            "slug": "bitcoin-above-100k",
            "question": "Will Bitcoin be above $100k?",
            "active": true,
            "closed": false,
            "liquidity": "1000.50",
            "volume": "2000.25",
            "outcomes": "[\"Yes\",\"No\"]",
            "outcomePrices": "[\"0.64\",\"0.36\"]",
            "clobTokenIds": "[\"token-yes\",\"token-no\"]"
        }]
    }));
    let config = PolymarketConfig::default()
        .with_enabled(true)
        .with_search_terms(instrument("BTC-USDT"), ["bitcoin"])
        .with_max_markets_per_term(2);
    let mut reader = PolymarketMcpReader::from_transport(transport, config).unwrap();

    let evidence = reader.collect_polymarket(&query()).unwrap();

    assert_eq!(evidence.len(), 1);
    let item = &evidence[0];
    assert_eq!(item.source, IntelligenceSource::Polymarket);
    assert_eq!(item.affected_assets[0].instrument, instrument("BTC-USDT"));
    assert_eq!(item.direction, IntelligenceDirection::Unclear);
    match &item.payload {
        EvidencePayload::PredictionMarket(market) => {
            assert_eq!(market.market_id, "market-1");
            assert_eq!(market.token_id, "token-yes");
            assert_eq!(
                market.implied_probability,
                Probability::new(Decimal::new(64, 2)).unwrap()
            );
            assert_eq!(market.liquidity, Some(Decimal::new(100050, 2)));
        }
        other => panic!("unexpected payload: {other:?}"),
    }
}

#[test]
fn polymarket_rest_reader_uses_gamma_search_and_clob_live_prices() {
    let transport = FakeRestTransport::new(vec![json!({
        "events": [{
            "markets": [{
                "id": "market-rest-1",
                "slug": "bitcoin-above-100k",
                "question": "Will Bitcoin be above $100k?",
                "active": true,
                "closed": false,
                "liquidity": "1000.50",
                "volume": "2000.25",
                "outcomes": "[\"Yes\",\"No\"]",
                "outcomePrices": "[\"0.64\",\"0.36\"]",
                "clobTokenIds": "[\"rest-token-yes\",\"rest-token-no\"]"
            }]
        }]
    })])
    .with_post_responses(vec![
        json!({"rest-token-yes": "0.66"}),
        json!({"rest-token-yes": "0.02"}),
    ]);
    let calls = Arc::clone(&transport.calls);
    let config = PolymarketConfig::default()
        .with_search_terms(instrument("BTC-USDT"), ["bitcoin"])
        .with_gamma_api_url("https://gamma.test")
        .with_clob_api_url("https://clob.test");
    let mut reader = PolymarketRestReader::from_transport(transport, config).unwrap();

    let evidence = reader.collect_polymarket(&query()).unwrap();

    assert_eq!(evidence.len(), 1);
    assert_eq!(
        calls.lock().unwrap().as_slice(),
        [
            "GET https://gamma.test/public-search",
            "POST https://clob.test/midpoints",
            "POST https://clob.test/spreads",
        ]
    );
    match &evidence[0].payload {
        EvidencePayload::PredictionMarket(market) => {
            assert_eq!(
                market.implied_probability,
                Probability::new(Decimal::new(66, 2)).unwrap()
            );
            assert_eq!(market.spread, Some(Decimal::new(2, 2)));
        }
        other => panic!("unexpected payload: {other:?}"),
    }
}

#[test]
fn polymarket_hybrid_reader_merges_rest_evidence_with_mcp_evidence() {
    let mcp_evidence = sample_evidence();
    let rest_evidence = IntelligenceEvidence {
        evidence_id: prooftrade::domain::EvidenceId::from_uuid(Uuid::from_u128(2)),
        source_event_id: SourceEventId::new("market-rest:token-yes").unwrap(),
        source_reference: Some(SourceReference::new("polymarket://market/market-rest").unwrap()),
        event_fingerprint: EventFingerprint::from_canonical_key("market-rest").unwrap(),
        payload: EvidencePayload::PredictionMarket(PredictionMarketEvidence {
            market_id: "market-rest".to_owned(),
            token_id: "token-yes".to_owned(),
            question: "Will Bitcoin move?".to_owned(),
            implied_probability: Probability::new(Decimal::new(55, 2)).unwrap(),
            probability_delta: None,
            liquidity: None,
            volume: None,
            spread: Some(Decimal::new(2, 2)),
        }),
        ..sample_evidence()
    };
    let mut reader = PolymarketHybridReader::new(
        Some(StaticReader {
            evidence: vec![mcp_evidence],
        }),
        Some(StaticReader {
            evidence: vec![rest_evidence],
        }),
    )
    .unwrap();

    let evidence = reader.collect_polymarket(&query()).unwrap();

    assert_eq!(evidence.len(), 2);
}

#[test]
fn polymarket_hybrid_reader_falls_back_when_mcp_fails() {
    let rest_evidence = sample_evidence();
    let mut reader = PolymarketHybridReader::new(
        Some(FailingReader),
        Some(StaticReader {
            evidence: vec![rest_evidence.clone()],
        }),
    )
    .unwrap();

    let evidence = reader.collect_polymarket(&query()).unwrap();

    assert_eq!(evidence.len(), 1);
    assert_eq!(evidence[0].source_event_id, rest_evidence.source_event_id);
}

#[test]
#[ignore = "requires the local polymarket-mcp executable"]
fn local_polymarket_mcp_process_supports_read_only_handshake() {
    let reader = PolymarketMcpReader::launch(
        PolymarketConfig::default().with_search_terms(instrument("BTC-USDT"), ["bitcoin"]),
    )
    .unwrap();

    assert!(reader.is_alive());
    reader.shutdown().unwrap();
}

#[test]
fn intelligence_service_is_a_research_provider_for_polymarket() {
    let evidence = sample_evidence();
    let mut service = IntelligenceService::new();
    service.register(PolymarketAdapter::new(FixtureReader {
        evidence: vec![evidence.clone()],
    }));
    let request = ResearchRequest::new(
        IntelligenceSource::Polymarket,
        instrument("BTC-USDT"),
        query(),
        "inspect the bounded prediction-market context",
    );

    let result = service.research(&request).unwrap();

    assert_eq!(result.source, IntelligenceSource::Polymarket);
    assert_eq!(result.evidence.len(), 1);
    assert_eq!(result.evidence[0].evidence_id, evidence.evidence_id);
    assert_eq!(result.evidence[0].source_event_id, evidence.source_event_id);
    assert!(result.evidence[0].lineage_id.is_some());
}

#[test]
fn orchestrator_exposes_a_runtime_polymarket_toggle() {
    let service = IntelligenceService::new();
    let mut orchestrator =
        DecisionOrchestrator::new(NoopPlanner, service, NoopAgent, Default::default());

    assert!(!orchestrator.polymarket_enabled());
    assert!(!orchestrator.disable_polymarket());
    assert!(!orchestrator.enable_polymarket());
}

fn query() -> BoundedEvidenceQuery {
    BoundedEvidenceQuery::new(now(), 10, 5)
        .unwrap()
        .with_instruments([instrument("BTC-USDT")])
}

fn now() -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap()
}

fn instrument(symbol: &str) -> Instrument {
    Instrument::parse(symbol).unwrap()
}

fn sample_evidence() -> IntelligenceEvidence {
    let source_event_id = SourceEventId::new("market-1:token-yes").unwrap();
    IntelligenceEvidence {
        evidence_id: prooftrade::domain::EvidenceId::from_uuid(Uuid::from_u128(1)),
        source: IntelligenceSource::Polymarket,
        source_event_id,
        event_time: None,
        observed_at: now(),
        affected_assets: vec![AssetImpact {
            instrument: instrument("BTC-USDT"),
            relevance: Score::new(Decimal::new(90, 2)).unwrap(),
        }],
        direction: IntelligenceDirection::Bullish,
        strength: Some(Score::new(Decimal::new(64, 2)).unwrap()),
        freshness: Some(Score::new(Decimal::ONE).unwrap()),
        reliability: Some(Score::new(Decimal::new(50, 2)).unwrap()),
        novelty: Some(Score::new(Decimal::new(50, 2)).unwrap()),
        summary: "Polymarket observation".to_owned(),
        source_reference: Some(SourceReference::new("polymarket://market/market-1").unwrap()),
        event_fingerprint: EventFingerprint::from_canonical_key("market-1").unwrap(),
        lineage_id: None,
        payload: EvidencePayload::PredictionMarket(PredictionMarketEvidence {
            market_id: "market-1".to_owned(),
            token_id: "token-yes".to_owned(),
            question: "Will Bitcoin be above $100k?".to_owned(),
            implied_probability: Probability::new(Decimal::new(64, 2)).unwrap(),
            probability_delta: Some(SignedValue::new(Decimal::new(4, 2)).unwrap()),
            liquidity: Some(Decimal::from(1_000)),
            volume: Some(Decimal::from(2_000)),
            spread: None,
        }),
    }
}

#[derive(Clone)]
struct FixtureReader {
    evidence: Vec<IntelligenceEvidence>,
}

#[derive(Clone)]
struct StaticReader {
    evidence: Vec<IntelligenceEvidence>,
}

struct FailingReader;

impl PolymarketReader for FailingReader {
    fn collect_polymarket(
        &mut self,
        _query: &BoundedEvidenceQuery,
    ) -> Result<Vec<IntelligenceEvidence>, prooftrade::intelligence::sources::SourceError> {
        Err(prooftrade::intelligence::sources::SourceError::new(
            IntelligenceSource::Polymarket,
            prooftrade::intelligence::SourceErrorCategory::Unavailable,
        ))
    }
}

impl PolymarketReader for StaticReader {
    fn collect_polymarket(
        &mut self,
        _query: &BoundedEvidenceQuery,
    ) -> Result<Vec<IntelligenceEvidence>, prooftrade::intelligence::sources::SourceError> {
        Ok(self.evidence.clone())
    }
}

struct FakeRestTransport {
    get_responses: Mutex<VecDeque<Value>>,
    post_responses: Mutex<VecDeque<Value>>,
    calls: Arc<Mutex<Vec<String>>>,
}

impl FakeRestTransport {
    fn new(get_responses: Vec<Value>) -> Self {
        Self {
            get_responses: Mutex::new(get_responses.into()),
            post_responses: Mutex::new(VecDeque::new()),
            calls: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn with_post_responses(self, responses: Vec<Value>) -> Self {
        *self.post_responses.lock().unwrap() = responses.into();
        self
    }
}

impl PolymarketRestTransport for FakeRestTransport {
    fn get_json(
        &mut self,
        url: &str,
        _query: &BTreeMap<String, String>,
    ) -> Result<Value, PolymarketRestError> {
        self.calls.lock().unwrap().push(format!("GET {url}"));
        self.get_responses
            .get_mut()
            .unwrap()
            .pop_front()
            .ok_or_else(|| PolymarketRestError::Request("missing fake GET response".to_owned()))
    }

    fn post_json(&mut self, url: &str, _body: Value) -> Result<Value, PolymarketRestError> {
        self.calls.lock().unwrap().push(format!("POST {url}"));
        self.post_responses
            .get_mut()
            .unwrap()
            .pop_front()
            .ok_or_else(|| PolymarketRestError::Request("missing fake POST response".to_owned()))
    }
}

struct NoopPlanner;

impl ResearchSourcePlanner for NoopPlanner {
    type Error = String;

    fn plan(
        &mut self,
        _context: &ResearchPlanningContext,
    ) -> Result<Vec<ResearchRequest>, Self::Error> {
        Ok(Vec::new())
    }
}

struct NoopAgent;

impl DecisionProvider for NoopAgent {
    type Error = String;

    fn decide(&mut self, _request: &AgentDecisionRequest) -> Result<Vec<String>, Self::Error> {
        Ok(Vec::new())
    }
}

impl PolymarketReader for FixtureReader {
    fn collect_polymarket(
        &mut self,
        _query: &BoundedEvidenceQuery,
    ) -> Result<Vec<IntelligenceEvidence>, prooftrade::intelligence::sources::SourceError> {
        Ok(self.evidence.clone())
    }
}

struct FakeMcpTransport {
    search_result: Mutex<VecDeque<Value>>,
}

impl FakeMcpTransport {
    fn with_search_result(result: Value) -> Self {
        Self {
            search_result: Mutex::new(VecDeque::from([tool_result(result)])),
        }
    }
}

impl McpTransport for FakeMcpTransport {
    fn request(&self, method: &str, _params: Value, _timeout: Duration) -> Result<Value, McpError> {
        match method {
            "initialize" => Ok(json!({
                "jsonrpc": "2.0",
                "id": 1,
                "result": {
                    "protocolVersion": "2024-11-05",
                    "serverInfo": {"name": "polymarket-mcp", "version": "0.2.0"}
                }
            })),
            "tools/list" => Ok(json!({
                "jsonrpc": "2.0",
                "id": 2,
                "result": {"tools": [{"name": "search_markets"}]}
            })),
            "tools/call" => self
                .search_result
                .lock()
                .unwrap()
                .pop_front()
                .ok_or(McpError::ProcessExited),
            _ => Err(McpError::InvalidEnvelope(format!(
                "unexpected method {method}"
            ))),
        }
    }

    fn notify(&self, _method: &str, _params: Value) -> Result<(), McpError> {
        Ok(())
    }

    fn is_alive(&self) -> bool {
        true
    }

    fn shutdown(&self) -> Result<(), McpError> {
        Ok(())
    }
}

fn tool_result(payload: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": 3,
        "result": {
            "content": [{
                "type": "text",
                "text": serde_json::to_string(&payload).unwrap()
            }]
        }
    })
}
