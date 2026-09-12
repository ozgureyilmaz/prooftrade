#![recursion_limit = "256"]

use prooftrade::domain::Instrument;
use prooftrade::intelligence::sources::reddit::{
    ProcessRedditReadOnlyTransport, RedditCryptoConfig, RedditCryptoReader,
    RedditReadOnlyTransport, RedditReader,
};
use prooftrade::intelligence::{
    BoundedEvidenceQuery, IntelligenceDirection, IntelligenceService, IntelligenceSource,
};
use serde_json::{Value, json};
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::time::Duration;
use time::OffsetDateTime;

struct FixtureTransport;

impl RedditReadOnlyTransport for FixtureTransport {
    fn detect_crypto(
        &mut self,
        _config: &RedditCryptoConfig,
        _query: &BoundedEvidenceQuery,
    ) -> Result<Value, prooftrade::intelligence::sources::SourceError> {
        Ok(json!({
            "command": "detect-crypto",
            "schemaVersion": "1.1",
            "executionMode": "LIVE_READ_ONLY",
            "dryRun": true,
            "publisherState": "DISABLED",
            "policy": {"pairs": ["BTC-USDT"], "subreddits": ["BitcoinMarkets"], "subredditsByPair": {"BTC-USDT": ["BitcoinMarkets"]}, "searchType": "all", "lookbackHours": 6, "perPairLimit": 5, "maxCandidates": 5, "maxRequests": 1, "minEventRelevance": 0.35},
            "sourceHealth": {"provider": "reddit", "status": "healthy"},
            "searches": [{"pair": "BTC-USDT", "query": "BTC", "subreddits": ["BitcoinMarkets"], "searchType": "all", "status": "healthy", "posts": 1, "filteredPosts": 0, "requestsUsed": 1, "skippedSubreddits": []}],
            "counts": {"discoveredPosts": 1, "filteredPosts": 0, "candidates": 1},
            "rejectedPosts": [],
            "candidates": [{
                "schemaVersion": "1.1",
                "untrustedContentNotice": true,
                "candidateId": "candidate-1",
                "pair": "BTC-USDT",
                "postId": "t3_post-1",
                "subreddit": "BitcoinMarkets",
                "permalink": "https://reddit.example/r/BitcoinMarkets/post-1",
                "title": "Bitcoin breakout event",
                "summary": "Bitcoin breakout event",
                "createdAt": "2026-09-12T09:00:00Z",
                "observedAt": "2026-09-12T09:01:00Z",
                "eventType": "MARKET_MOVE",
                "eventTypes": ["MARKET_MOVE"],
                "direction": "BULLISH",
                "eventRelevance": 0.82,
                "assetMatches": ["bitcoin"],
                "eventMatches": ["breakout"],
                "supportingSignals": ["event:breakout"],
                "counterSignals": ["bearish:volatility"],
                "crossCommunitySubreddits": ["BitcoinMarkets"],
                "crossCommunityConfirmation": 0.0,
                "eventFingerprint": "crypto-event-1",
                "detectorRuleVersion": "crypto-detector/v2",
                "context": {"status": "COMPLETE", "commentsScanned": 3, "uniqueAuthors": 2, "supportingSignals": ["comment:c1:bullish:breakout"], "counterSignals": ["comment:c2:bearish:volatility"]}
            }],
            "actions": []
        }))
    }
}

#[test]
fn reddit_read_only_candidate_maps_to_trading_evidence_without_actions() {
    let query = BoundedEvidenceQuery::new(
        OffsetDateTime::parse(
            "2026-09-12T10:00:00Z",
            &time::format_description::well_known::Rfc3339,
        )
        .unwrap(),
        5,
        5,
    )
    .unwrap()
    .with_instruments([Instrument::parse("BTC-USDT").unwrap()]);
    let config = RedditCryptoConfig::new(".").unwrap();
    let mut reader = RedditCryptoReader::from_transport(FixtureTransport, config);

    let evidence = reader.collect_reddit(&query).unwrap();

    assert_eq!(evidence.len(), 1);
    assert_eq!(evidence[0].direction, IntelligenceDirection::Bullish);
    assert_eq!(
        evidence[0].source,
        prooftrade::intelligence::IntelligenceSource::Reddit
    );
    assert!(matches!(
        &evidence[0].payload,
        prooftrade::intelligence::EvidencePayload::Reddit(_)
    ));
    assert_eq!(
        evidence[0].source_event_id.as_str(),
        "reddit:BTC-USDT:t3_post-1"
    );
    assert!(evidence[0].freshness.is_none());
    assert!(evidence[0].reliability.is_none());
    let prooftrade::intelligence::EvidencePayload::Reddit(reddit) = &evidence[0].payload else {
        unreachable!()
    };
    assert_eq!(reddit.event_type.as_deref(), Some("MARKET_MOVE"));
    assert_eq!(
        reddit.supporting_signals,
        vec!["event:breakout", "comment:c1:bullish:breakout"]
    );
    assert_eq!(
        reddit.counter_signals,
        vec!["bearish:volatility", "comment:c2:bearish:volatility"]
    );
    assert_eq!(reddit.context_comments, Some(3));
}

#[test]
fn reddit_reader_rejects_a_non_read_only_detector_response() {
    struct UnsafeTransport;
    impl RedditReadOnlyTransport for UnsafeTransport {
        fn detect_crypto(
            &mut self,
            _config: &RedditCryptoConfig,
            _query: &BoundedEvidenceQuery,
        ) -> Result<Value, prooftrade::intelligence::sources::SourceError> {
            Ok(json!({
                "command": "detect-crypto",
                "schemaVersion": "1.1",
                "executionMode": "LIVE_READ_ONLY",
                "dryRun": true,
                "publisherState": "AUTHORIZED",
                "candidates": [],
                "actions": []
            }))
        }
    }
    let query = BoundedEvidenceQuery::new(
        OffsetDateTime::parse(
            "2026-09-12T10:00:00Z",
            &time::format_description::well_known::Rfc3339,
        )
        .unwrap(),
        1,
        1,
    )
    .unwrap();
    let config = RedditCryptoConfig::new(".").unwrap();
    let mut reader = RedditCryptoReader::from_transport(UnsafeTransport, config);
    assert!(reader.collect_reddit(&query).is_err());
}

#[test]
fn process_transport_times_out_without_waiting_for_a_hung_detector() {
    let script_path = std::env::temp_dir().join(format!(
        "prooftrade-reddit-timeout-{}",
        uuid::Uuid::new_v4()
    ));
    std::fs::write(&script_path, "#!/bin/sh\nsleep 2\n").unwrap();
    let mut permissions = std::fs::metadata(&script_path).unwrap().permissions();
    permissions.set_mode(0o700);
    std::fs::set_permissions(&script_path, permissions).unwrap();
    let config = RedditCryptoConfig::new(PathBuf::from("."))
        .unwrap()
        .with_executable(script_path.to_string_lossy())
        .with_process_timeout(Duration::from_millis(25));
    let query = BoundedEvidenceQuery::new(
        OffsetDateTime::parse(
            "2026-09-12T10:00:00Z",
            &time::format_description::well_known::Rfc3339,
        )
        .unwrap(),
        1,
        1,
    )
    .unwrap();
    let result = ProcessRedditReadOnlyTransport.detect_crypto(&config, &query);
    let _ = std::fs::remove_file(script_path);
    assert!(
        matches!(result, Err(error) if error.category() == prooftrade::intelligence::SourceErrorCategory::Timeout)
    );
}

#[test]
fn intelligence_service_can_register_the_read_only_reddit_detector() {
    let mut service = IntelligenceService::new();
    service.register_reddit_crypto(RedditCryptoConfig::new(".").unwrap());
    assert!(service.is_source_enabled(IntelligenceSource::Reddit));
}
