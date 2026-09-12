use prooftrade::domain::{EvidenceId, Instrument, LineageId, SourceEventId};
use prooftrade::intelligence::sources::news::{NewsAdapter, NewsCapability};
use prooftrade::intelligence::sources::polymarket::ProbabilityHistory;
use prooftrade::intelligence::sources::{IntelligenceSourceAdapter, SourceError};
use prooftrade::intelligence::{
    AssetImpact, EventFingerprint, EventIdentity, EvidencePayload, FreshnessPolicy,
    IngestDisposition, IntelligenceDirection, IntelligenceEvidence, IntelligenceSource,
    LineageLedger, MarxEvidence, NewsEvidence, PredictionMarketEvidence, Probability,
    RedditEvidence, Score, SignedValue, SourceErrorCategory, SourceHealthRegistry,
    SourceHealthStatus, SourceReference,
};
use prooftrade::intelligence::{BoundedEvidenceQuery, IntelligenceService};
use rust_decimal::Decimal;
use time::OffsetDateTime;
use uuid::Uuid;

fn timestamp(seconds: i64) -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(seconds).unwrap()
}

fn evidence_id(value: u128) -> EvidenceId {
    EvidenceId::from_uuid(Uuid::from_u128(value))
}

fn lineage_id(value: u128) -> LineageId {
    LineageId::from_uuid(Uuid::from_u128(value))
}

fn news_evidence(direction: IntelligenceDirection) -> IntelligenceEvidence {
    let btc = Instrument::parse("BTC-USDT").unwrap();
    IntelligenceEvidence {
        evidence_id: evidence_id(10),
        source: IntelligenceSource::AtkNews,
        source_event_id: SourceEventId::new("atk-news-1").unwrap(),
        event_time: Some(timestamp(1_700_000_000)),
        observed_at: timestamp(1_700_000_060),
        affected_assets: vec![AssetImpact {
            instrument: btc,
            relevance: Score::new(Decimal::new(94, 2)).unwrap(),
        }],
        direction,
        strength: Some(Score::new(Decimal::new(82, 2)).unwrap()),
        freshness: Some(Score::new(Decimal::new(98, 2)).unwrap()),
        reliability: Some(Score::new(Decimal::new(90, 2)).unwrap()),
        novelty: Some(Score::new(Decimal::new(80, 2)).unwrap()),
        summary: "regulatory approval reported".to_owned(),
        source_reference: Some(SourceReference::new("https://example.test/news/1").unwrap()),
        event_fingerprint: EventFingerprint::from_canonical_key("approval|sec|btc|1700000000")
            .unwrap(),
        lineage_id: Some(lineage_id(11)),
        payload: EvidencePayload::News(NewsEvidence {
            headline: "Approval reported".to_owned(),
            summary: Some("A source reported the approval.".to_owned()),
            category: Some("regulatory".to_owned()),
            importance: Some(Score::new(Decimal::new(90, 2)).unwrap()),
            published_at: Some(timestamp(1_700_000_000)),
        }),
    }
}

#[test]
fn normalized_evidence_preserves_source_payload_and_distinct_times() {
    let evidence = news_evidence(IntelligenceDirection::Bullish);

    evidence.validate().unwrap();

    assert_eq!(evidence.source, IntelligenceSource::AtkNews);
    assert_ne!(evidence.event_time, Some(evidence.observed_at));
    assert!(matches!(evidence.payload, EvidencePayload::News(_)));
}

#[test]
fn normalized_scores_reject_values_above_one() {
    let error = Score::new(Decimal::new(101, 2)).unwrap_err();

    assert!(matches!(
        error,
        prooftrade::intelligence::IntelligenceError::ScoreOutOfRange { .. }
    ));
}

#[test]
fn bearish_evidence_remains_counter_direction_after_validation() {
    let evidence = news_evidence(IntelligenceDirection::Bearish);

    evidence.validate().unwrap();

    assert_eq!(evidence.direction, IntelligenceDirection::Bearish);
}

#[test]
fn prediction_market_payload_preserves_probability_and_delta() {
    let mut evidence = news_evidence(IntelligenceDirection::Bullish);
    evidence.source = IntelligenceSource::Polymarket;
    evidence.payload = EvidencePayload::PredictionMarket(PredictionMarketEvidence {
        market_id: "market-1".to_owned(),
        token_id: "token-yes".to_owned(),
        question: "Will the event resolve positively?".to_owned(),
        implied_probability: Probability::new(Decimal::new(63, 2)).unwrap(),
        probability_delta: Some(SignedValue::new(Decimal::new(8, 2)).unwrap()),
        liquidity: Some(Decimal::new(10_000, 0)),
        volume: Some(Decimal::new(25_000, 0)),
        spread: Some(Decimal::new(2, 2)),
    });

    evidence.validate().unwrap();

    let EvidencePayload::PredictionMarket(payload) = evidence.payload else {
        panic!("expected Polymarket payload");
    };
    assert_eq!(payload.implied_probability.value(), Decimal::new(63, 2));
    assert_eq!(
        payload.probability_delta.unwrap().value(),
        Decimal::new(8, 2)
    );
}

#[test]
fn source_and_payload_mismatch_is_rejected() {
    let mut evidence = news_evidence(IntelligenceDirection::Bullish);
    evidence.source = IntelligenceSource::Reddit;

    let error = evidence.validate().unwrap_err();

    assert!(matches!(
        error,
        prooftrade::intelligence::IntelligenceError::PayloadSourceMismatch
    ));
}

#[test]
fn disagreeing_marx_theses_remain_distinct_evidence_records() {
    let mut bullish = news_evidence(IntelligenceDirection::Bullish);
    bullish.evidence_id = evidence_id(20);
    bullish.source = IntelligenceSource::MarxFinance;
    bullish.source_event_id = SourceEventId::new("marx-a").unwrap();
    bullish.payload = EvidencePayload::Marx(MarxEvidence {
        agent_id: "agent-a".to_owned(),
        thesis: "approval supports demand".to_owned(),
        stance: IntelligenceDirection::Bullish,
        supporting_arguments: vec!["fresh catalyst".to_owned()],
        counter_arguments: vec![],
        confidence: Some(Score::new(Decimal::new(74, 2)).unwrap()),
    });

    let mut bearish = bullish.clone();
    bearish.evidence_id = evidence_id(21);
    bearish.source_event_id = SourceEventId::new("marx-b").unwrap();
    bearish.event_fingerprint = EventFingerprint::from_canonical_key("same-event").unwrap();
    bearish.payload = EvidencePayload::Marx(MarxEvidence {
        agent_id: "agent-b".to_owned(),
        thesis: "approval is already priced".to_owned(),
        stance: IntelligenceDirection::Bearish,
        supporting_arguments: vec!["spot already moved".to_owned()],
        counter_arguments: vec![],
        confidence: Some(Score::new(Decimal::new(68, 2)).unwrap()),
    });

    bullish.validate().unwrap();
    bearish.validate().unwrap();

    assert_ne!(bullish.evidence_id, bearish.evidence_id);
    assert_ne!(bullish.payload, bearish.payload);
}

#[test]
fn event_fingerprint_is_stable_when_identity_inputs_are_reordered() {
    let btc = Instrument::parse("BTC-USDT").unwrap();
    let eth = Instrument::parse("ETH-USDT").unwrap();
    let first = EventIdentity::new(
        "regulatory_approval",
        "sec approval",
        vec!["SEC".to_owned(), "BTC".to_owned()],
        vec![btc.clone(), eth.clone()],
        Some(timestamp(1_700_000_000)),
        300,
    )
    .unwrap();
    let second = EventIdentity::new(
        " regulatory_approval ",
        "SEC APPROVAL",
        vec!["BTC".to_owned(), "SEC".to_owned()],
        vec![eth, btc],
        Some(timestamp(1_700_000_099)),
        300,
    )
    .unwrap();

    assert_eq!(first.fingerprint().unwrap(), second.fingerprint().unwrap());
}

#[test]
fn repeated_source_event_updates_one_observation_without_new_catalyst() {
    let first = news_evidence(IntelligenceDirection::Bullish);
    let first_id = first.evidence_id;
    let first_fingerprint = first.event_fingerprint.clone();
    let mut ledger = LineageLedger::new();

    let first_result = ledger.ingest(first).unwrap();
    let mut updated = news_evidence(IntelligenceDirection::Bullish);
    updated.evidence_id = evidence_id(12);
    updated.observed_at = timestamp(1_700_000_120);
    updated.summary = "updated regulatory approval summary".to_owned();
    updated.event_fingerprint = first_fingerprint;
    let second_result = ledger.ingest(updated).unwrap();

    assert_eq!(ledger.observations().len(), 1);
    assert_eq!(ledger.observations()[0].evidence_id, first_id);
    assert_eq!(
        ledger.observations()[0].summary,
        "updated regulatory approval summary"
    );
    assert!(matches!(
        first_result.disposition,
        IngestDisposition::NewCatalyst
    ));
    assert!(matches!(
        second_result.disposition,
        IngestDisposition::UpdatedSourceEvent
    ));
    assert_eq!(first_result.lineage_id, second_result.lineage_id);
}

#[test]
fn cross_source_reaction_shares_lineage_without_erasing_evidence() {
    let news = news_evidence(IntelligenceDirection::Bullish);
    let fingerprint = news.event_fingerprint.clone();
    let mut reddit = news.clone();
    reddit.evidence_id = evidence_id(13);
    reddit.source = IntelligenceSource::Reddit;
    reddit.source_event_id = SourceEventId::new("reddit-post-1").unwrap();
    reddit.payload = EvidencePayload::Reddit(RedditEvidence {
        post_id: "reddit-post-1".to_owned(),
        communities: vec!["Bitcoin".to_owned(), "CryptoCurrency".to_owned()],
        event_type: Some("MARKET_MOVE".to_owned()),
        supporting_signals: vec!["event:breakout".to_owned()],
        counter_signals: vec![],
        context_comments: Some(0),
        event_relevance: Score::new(Decimal::new(91, 2)).unwrap(),
        mention_velocity: Some(Decimal::new(280, 0)),
        unique_author_velocity: Some(Decimal::new(205, 0)),
        engagement_velocity: Some(Decimal::new(164, 0)),
        sentiment: Some(SignedValue::new(Decimal::new(59, 2)).unwrap()),
        sentiment_delta: Some(SignedValue::new(Decimal::new(31, 2)).unwrap()),
        sentiment_dispersion: Some(Score::new(Decimal::new(22, 2)).unwrap()),
        cross_community_confirmation: Some(Score::new(Decimal::new(78, 2)).unwrap()),
    });
    reddit.event_fingerprint = fingerprint;

    let mut ledger = LineageLedger::new();
    let news_result = ledger.ingest(news).unwrap();
    let reddit_result = ledger.ingest(reddit).unwrap();

    assert_eq!(ledger.observations().len(), 2);
    assert_eq!(news_result.lineage_id, reddit_result.lineage_id);
    assert!(matches!(
        reddit_result.disposition,
        IngestDisposition::RelatedEvidence
    ));
}

#[test]
fn source_timeout_degrades_only_the_failed_source() {
    let mut health = SourceHealthRegistry::new();
    let now = timestamp(1_700_000_120);

    health.record_success(IntelligenceSource::AtkNews, now);
    health.record_success(IntelligenceSource::Polymarket, now);
    health.record_failure(
        IntelligenceSource::Reddit,
        now,
        SourceErrorCategory::Timeout,
    );

    assert_eq!(
        health.status(IntelligenceSource::Reddit),
        SourceHealthStatus::Degraded
    );
    assert_eq!(
        health.status(IntelligenceSource::AtkNews),
        SourceHealthStatus::Healthy
    );
    assert_eq!(
        health.status(IntelligenceSource::Polymarket),
        SourceHealthStatus::Healthy
    );
    assert_eq!(
        health.status(IntelligenceSource::MarxFinance),
        SourceHealthStatus::Unknown
    );
}

#[test]
fn successful_empty_result_recovers_source_without_creating_evidence() {
    let mut health = SourceHealthRegistry::new();
    let now = timestamp(1_700_000_120);

    health.record_success(IntelligenceSource::Reddit, now);

    assert_eq!(
        health.status(IntelligenceSource::Reddit),
        SourceHealthStatus::Healthy
    );
    assert!(health.last_error(IntelligenceSource::Reddit).is_none());
}

#[test]
fn freshness_uses_event_time_not_repeated_observation_time() {
    let policy = FreshnessPolicy::linear(600).unwrap();
    let event_time = timestamp(1_700_000_000);
    let evaluation_time = timestamp(1_700_000_300);

    let first = policy
        .calculate(event_time, timestamp(1_700_000_001), evaluation_time)
        .unwrap();
    let repeated = policy
        .calculate(event_time, timestamp(1_700_000_299), evaluation_time)
        .unwrap();

    assert_eq!(first, repeated);
    assert_eq!(first.value(), Decimal::new(50, 2));
}

#[test]
fn future_event_time_is_rejected_by_freshness_calculation() {
    let policy = FreshnessPolicy::linear(600).unwrap();

    let error = policy
        .calculate(
            timestamp(1_700_001_000),
            timestamp(1_700_000_000),
            timestamp(1_700_000_300),
        )
        .unwrap_err();

    assert!(matches!(
        error,
        prooftrade::intelligence::IntelligenceError::FutureObservation
    ));
}

#[test]
fn polymarket_history_calculates_probability_points_not_percent_change() {
    let mut history = ProbabilityHistory::new(4).unwrap();
    let market = "market-1";

    assert!(
        history
            .record(
                market,
                timestamp(1_700_000_000),
                Probability::new(Decimal::new(55, 2)).unwrap(),
            )
            .unwrap()
            .is_none()
    );
    let delta = history
        .record(
            market,
            timestamp(1_700_000_060),
            Probability::new(Decimal::new(63, 2)).unwrap(),
        )
        .unwrap()
        .unwrap();

    assert_eq!(delta.value(), Decimal::new(8, 2));
}

#[test]
fn polymarket_history_does_not_fabricate_delta_without_previous_observation() {
    let mut history = ProbabilityHistory::new(4).unwrap();

    let delta = history
        .record(
            "market-1",
            timestamp(1_700_000_000),
            Probability::new(Decimal::new(63, 2)).unwrap(),
        )
        .unwrap();

    assert!(delta.is_none());
}

struct FixtureSource {
    source: IntelligenceSource,
    result: Result<Vec<IntelligenceEvidence>, SourceError>,
}

impl IntelligenceSourceAdapter for FixtureSource {
    fn source(&self) -> IntelligenceSource {
        self.source
    }

    fn collect(
        &mut self,
        _query: &BoundedEvidenceQuery,
    ) -> Result<Vec<IntelligenceEvidence>, SourceError> {
        self.result.clone()
    }
}

#[test]
fn intelligence_service_bounds_results_and_isolates_source_failure() {
    let mut first = news_evidence(IntelligenceDirection::Bullish);
    first.observed_at = timestamp(1_700_000_090);
    let mut second = news_evidence(IntelligenceDirection::Bearish);
    second.evidence_id = evidence_id(30);
    second.source_event_id = SourceEventId::new("atk-news-2").unwrap();
    second.observed_at = timestamp(1_700_000_100);
    let failing = FixtureSource {
        source: IntelligenceSource::Reddit,
        result: Err(SourceError::new(
            IntelligenceSource::Reddit,
            SourceErrorCategory::Timeout,
        )),
    };
    let mut service = IntelligenceService::new();
    service.register(FixtureSource {
        source: IntelligenceSource::AtkNews,
        result: Ok(vec![second, first]),
    });
    service.register(failing);

    let query = BoundedEvidenceQuery::new(timestamp(1_700_000_120), 1, 1).unwrap();
    let snapshot = service.collect(&query).unwrap();

    assert_eq!(snapshot.observations().len(), 1);
    assert_eq!(
        snapshot.source_health().status(IntelligenceSource::Reddit),
        SourceHealthStatus::Degraded
    );
    assert_eq!(snapshot.source_errors().len(), 1);
    assert!(snapshot.was_truncated());
}

#[test]
fn malformed_source_observation_degrades_source_health() {
    let mut invalid = news_evidence(IntelligenceDirection::Bullish);
    invalid.summary.clear();
    let mut service = IntelligenceService::new();
    service.register(FixtureSource {
        source: IntelligenceSource::AtkNews,
        result: Ok(vec![invalid]),
    });

    let query = BoundedEvidenceQuery::new(timestamp(1_700_000_120), 2, 2).unwrap();
    let snapshot = service.collect(&query).unwrap();

    assert_eq!(snapshot.observations().len(), 0);
    assert_eq!(snapshot.source_errors().len(), 1);
    assert_eq!(
        snapshot.source_health().status(IntelligenceSource::AtkNews),
        SourceHealthStatus::Degraded
    );
}

#[test]
fn missing_atk_news_capability_becomes_explicit_source_failure() {
    let mut adapter = NewsAdapter::<()>::without_reader(NewsCapability::Missing);
    let query = BoundedEvidenceQuery::new(timestamp(1_700_000_120), 2, 2).unwrap();

    let error = adapter.collect(&query).unwrap_err();

    assert_eq!(error.category(), SourceErrorCategory::CapabilityMissing);
}
