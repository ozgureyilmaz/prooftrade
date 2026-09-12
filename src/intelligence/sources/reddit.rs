use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

use serde::Deserialize;
use serde_json::Value;
use time::OffsetDateTime;
use uuid::Uuid;
use wait_timeout::ChildExt;

use super::{IntelligenceSourceAdapter, SourceError};
use crate::domain::{EvidenceId, Instrument, SourceEventId};
use crate::intelligence::{
    AssetImpact, BoundedEvidenceQuery, EventFingerprint, EvidencePayload, IntelligenceDirection,
    IntelligenceError, IntelligenceEvidence, IntelligenceSource, RedditEvidence, Score,
    SourceErrorCategory, SourceReference,
};

pub trait RedditReader {
    fn collect_reddit(
        &mut self,
        query: &BoundedEvidenceQuery,
    ) -> Result<Vec<IntelligenceEvidence>, SourceError>;
}

pub struct RedditAdapter<R> {
    reader: R,
}

impl<R> RedditAdapter<R> {
    pub fn new(reader: R) -> Self {
        Self { reader }
    }
}

impl<R: RedditReader> IntelligenceSourceAdapter for RedditAdapter<R> {
    fn source(&self) -> IntelligenceSource {
        IntelligenceSource::Reddit
    }

    fn collect(
        &mut self,
        query: &BoundedEvidenceQuery,
    ) -> Result<Vec<IntelligenceEvidence>, SourceError> {
        self.reader.collect_reddit(query)
    }
}

#[derive(Clone, Debug)]
pub struct RedditCryptoConfig {
    pub executable: String,
    pub working_dir: PathBuf,
    pub db_path: PathBuf,
    pub subreddit_profile: String,
    pub lookback_hours: u64,
    pub min_event_relevance: Score,
    pub process_timeout: Duration,
    pub max_output_bytes: usize,
}

impl RedditCryptoConfig {
    pub fn new(working_dir: impl Into<PathBuf>) -> Result<Self, IntelligenceError> {
        Ok(Self {
            executable: "npm".to_owned(),
            working_dir: working_dir.into(),
            db_path: PathBuf::from(".data/prooftrade-crypto-read.sqlite"),
            subreddit_profile: "prooftrade-crypto".to_owned(),
            lookback_hours: 6,
            min_event_relevance: Score::new(rust_decimal::Decimal::new(35, 2))?,
            process_timeout: Duration::from_secs(60),
            max_output_bytes: 2 * 1024 * 1024,
        })
    }

    pub fn with_executable(mut self, executable: impl Into<String>) -> Self {
        self.executable = executable.into();
        self
    }

    pub fn with_db_path(mut self, db_path: impl Into<PathBuf>) -> Self {
        self.db_path = db_path.into();
        self
    }

    pub fn with_subreddit_profile(mut self, profile: impl Into<String>) -> Self {
        self.subreddit_profile = profile.into();
        self
    }

    pub fn with_lookback_hours(mut self, lookback_hours: u64) -> Self {
        self.lookback_hours = lookback_hours;
        self
    }

    pub fn with_min_event_relevance(mut self, relevance: Score) -> Self {
        self.min_event_relevance = relevance;
        self
    }

    pub fn with_process_timeout(mut self, timeout: Duration) -> Self {
        self.process_timeout = timeout;
        self
    }

    pub fn with_max_output_bytes(mut self, max_output_bytes: usize) -> Self {
        self.max_output_bytes = max_output_bytes;
        self
    }
}

pub trait RedditReadOnlyTransport {
    fn detect_crypto(
        &mut self,
        config: &RedditCryptoConfig,
        query: &BoundedEvidenceQuery,
    ) -> Result<Value, SourceError>;
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RedditDetectionResponse {
    command: String,
    schema_version: String,
    execution_mode: String,
    dry_run: bool,
    publisher_state: String,
    policy: Value,
    source_health: Value,
    searches: Value,
    counts: Value,
    candidates: Vec<RedditCandidate>,
    rejected_posts: Vec<Value>,
    actions: Vec<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RedditCandidate {
    schema_version: String,
    untrusted_content_notice: bool,
    candidate_id: String,
    pair: String,
    post_id: String,
    subreddit: String,
    permalink: String,
    title: String,
    summary: String,
    created_at: String,
    observed_at: String,
    event_type: String,
    event_types: Vec<String>,
    direction: String,
    event_relevance: f64,
    asset_matches: Vec<String>,
    event_matches: Vec<String>,
    supporting_signals: Vec<String>,
    counter_signals: Vec<String>,
    cross_community_subreddits: Vec<String>,
    cross_community_confirmation: f64,
    event_fingerprint: String,
    detector_rule_version: String,
    context: RedditCandidateContext,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RedditCandidateContext {
    status: String,
    comments_scanned: u32,
    _unique_authors: u32,
    supporting_signals: Vec<String>,
    counter_signals: Vec<String>,
}

#[derive(Default)]
pub struct ProcessRedditReadOnlyTransport;

impl RedditReadOnlyTransport for ProcessRedditReadOnlyTransport {
    fn detect_crypto(
        &mut self,
        config: &RedditCryptoConfig,
        query: &BoundedEvidenceQuery,
    ) -> Result<Value, SourceError> {
        let pairs = if query.instruments().is_empty() {
            ["BTC-USDT", "ETH-USDT", "SOL-USDT", "HYPE-USDT"].join(",")
        } else {
            query
                .instruments()
                .iter()
                .map(Instrument::symbol)
                .collect::<Vec<_>>()
                .join(",")
        };
        let lookback_hours = query
            .max_age_seconds()
            .map(|seconds| ((seconds.max(0) + 3_599) / 3_600).max(1) as u64)
            .unwrap_or(config.lookback_hours);
        let mut child = Command::new(&config.executable)
            .current_dir(&config.working_dir)
            .args([
                "--silent",
                "run",
                "dev",
                "--",
                "detect-crypto",
                "--mode",
                "LIVE_READ_ONLY",
                "--pairs",
                pairs.as_str(),
                "--subreddit-profile",
                config.subreddit_profile.as_str(),
                "--lookback-hours",
                &lookback_hours.to_string(),
                "--max-candidates",
                &query.max_results().to_string(),
                "--min-event-relevance",
                &config.min_event_relevance.value().to_string(),
                "--db",
                config.db_path.to_string_lossy().as_ref(),
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| {
                SourceError::new(IntelligenceSource::Reddit, SourceErrorCategory::Unavailable)
            })?;
        let status = match child.wait_timeout(config.process_timeout).map_err(|_| {
            SourceError::new(
                IntelligenceSource::Reddit,
                SourceErrorCategory::ProcessFailure,
            )
        })? {
            Some(status) => status,
            None => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(SourceError::new(
                    IntelligenceSource::Reddit,
                    SourceErrorCategory::Timeout,
                ));
            }
        };
        let mut stdout = child.stdout.take().ok_or_else(|| {
            SourceError::new(
                IntelligenceSource::Reddit,
                SourceErrorCategory::ProcessFailure,
            )
        })?;
        let mut bytes = Vec::new();
        std::io::Read::read_to_end(&mut stdout, &mut bytes).map_err(|_| {
            SourceError::new(
                IntelligenceSource::Reddit,
                SourceErrorCategory::ProcessFailure,
            )
        })?;
        if bytes.len() > config.max_output_bytes {
            return Err(SourceError::new(
                IntelligenceSource::Reddit,
                SourceErrorCategory::MalformedResponse,
            ));
        }
        if !status.success() {
            return Err(SourceError::new(
                IntelligenceSource::Reddit,
                SourceErrorCategory::ProcessFailure,
            ));
        }
        serde_json::from_slice(&bytes).map_err(|_| {
            SourceError::new(
                IntelligenceSource::Reddit,
                SourceErrorCategory::MalformedResponse,
            )
        })
    }
}

pub struct RedditCryptoReader<T = ProcessRedditReadOnlyTransport> {
    transport: T,
    config: RedditCryptoConfig,
}

const REDDIT_EVENT_TYPES: [&str; 11] = [
    "EXPLOIT",
    "OUTAGE",
    "LISTING",
    "DELISTING",
    "UNLOCK",
    "UPGRADE",
    "REGULATORY",
    "FLOW",
    "LIQUIDATION",
    "ADOPTION",
    "MARKET_MOVE",
];

impl RedditCryptoReader<ProcessRedditReadOnlyTransport> {
    pub fn from_config(config: RedditCryptoConfig) -> Self {
        Self {
            transport: ProcessRedditReadOnlyTransport,
            config,
        }
    }
}

impl<T: RedditReadOnlyTransport> RedditCryptoReader<T> {
    pub fn from_transport(transport: T, config: RedditCryptoConfig) -> Self {
        Self { transport, config }
    }
}

impl<T: RedditReadOnlyTransport> RedditReader for RedditCryptoReader<T> {
    fn collect_reddit(
        &mut self,
        query: &BoundedEvidenceQuery,
    ) -> Result<Vec<IntelligenceEvidence>, SourceError> {
        let output = self.transport.detect_crypto(&self.config, query)?;
        let response = parse_detection_response(output)?;
        let mut evidence = Vec::new();
        for candidate in response.candidates.iter().take(query.max_results()) {
            if let Some(item) = candidate_to_evidence(candidate)?
                && query.matches(&item)
            {
                evidence.push(item);
            }
        }
        Ok(evidence)
    }
}

fn parse_detection_response(value: Value) -> Result<RedditDetectionResponse, SourceError> {
    let response: RedditDetectionResponse = serde_json::from_value(value).map_err(|_| {
        SourceError::new(
            IntelligenceSource::Reddit,
            SourceErrorCategory::MalformedResponse,
        )
    })?;
    if response.command != "detect-crypto"
        || response.schema_version != "1.1"
        || response.execution_mode != "LIVE_READ_ONLY"
        || !response.dry_run
        || response.publisher_state != "DISABLED"
        || !response.actions.is_empty()
        || !response.policy.is_object()
        || !response.source_health.is_object()
        || !response.searches.is_array()
        || !response.counts.is_object()
        || !response.rejected_posts.iter().all(Value::is_object)
    {
        return Err(SourceError::new(
            IntelligenceSource::Reddit,
            SourceErrorCategory::InvalidData,
        ));
    }
    for candidate in &response.candidates {
        if candidate.schema_version != "1.1"
            || !candidate.untrusted_content_notice
            || candidate.candidate_id.trim().is_empty()
            || candidate.pair.trim().is_empty()
            || candidate.post_id.trim().is_empty()
            || candidate.subreddit.trim().is_empty()
            || candidate.title.trim().is_empty()
            || candidate.summary.trim().is_empty()
            || candidate.event_type.trim().is_empty()
            || candidate.event_types.is_empty()
            || !REDDIT_EVENT_TYPES.contains(&candidate.event_type.as_str())
            || candidate
                .event_types
                .iter()
                .any(|event_type| !REDDIT_EVENT_TYPES.contains(&event_type.as_str()))
            || candidate.asset_matches.is_empty()
            || candidate.event_matches.is_empty()
            || candidate.cross_community_subreddits.is_empty()
            || !candidate.permalink.starts_with("https://")
            || !candidate.event_relevance.is_finite()
            || !(0.0..=1.0).contains(&candidate.event_relevance)
            || !candidate.cross_community_confirmation.is_finite()
            || !(0.0..=1.0).contains(&candidate.cross_community_confirmation)
            || candidate.detector_rule_version != "crypto-detector/v2"
            || !["BULLISH", "BEARISH", "MIXED", "UNCLEAR"].contains(&candidate.direction.as_str())
            || !["NOT_REQUESTED", "COMPLETE", "UNAVAILABLE"]
                .contains(&candidate.context.status.as_str())
        {
            return Err(SourceError::new(
                IntelligenceSource::Reddit,
                SourceErrorCategory::InvalidData,
            ));
        }
    }
    Ok(response)
}

fn candidate_to_evidence(
    candidate: &RedditCandidate,
) -> Result<Option<IntelligenceEvidence>, SourceError> {
    let instrument = Instrument::parse(&candidate.pair).map_err(|_| {
        SourceError::new(IntelligenceSource::Reddit, SourceErrorCategory::InvalidData)
    })?;
    let observed_at = parse_timestamp_str(&candidate.observed_at)?;
    let event_time = parse_timestamp_str(&candidate.created_at)?;
    let relevance = decimal_score_value(candidate.event_relevance)?;
    let direction = match candidate.direction.as_str() {
        "BULLISH" => IntelligenceDirection::Bullish,
        "BEARISH" => IntelligenceDirection::Bearish,
        "MIXED" => IntelligenceDirection::Mixed,
        _ => IntelligenceDirection::Unclear,
    };
    let event_fingerprint = EventFingerprint::from_canonical_key(&candidate.event_fingerprint)
        .map_err(|_| {
            SourceError::new(IntelligenceSource::Reddit, SourceErrorCategory::InvalidData)
        })?;
    let source_event_id =
        SourceEventId::new(format!("reddit:{}:{}", candidate.pair, candidate.post_id)).map_err(
            |_| SourceError::new(IntelligenceSource::Reddit, SourceErrorCategory::InvalidData),
        )?;
    Ok(Some(IntelligenceEvidence {
        evidence_id: EvidenceId::from_uuid(Uuid::new_v4()),
        source: IntelligenceSource::Reddit,
        source_event_id,
        event_time: Some(event_time),
        observed_at,
        affected_assets: vec![AssetImpact {
            instrument,
            relevance,
        }],
        direction,
        strength: Some(relevance),
        freshness: None,
        reliability: None,
        novelty: None,
        summary: candidate.summary.clone(),
        source_reference: Some(SourceReference::new(&candidate.permalink).map_err(|_| {
            SourceError::new(IntelligenceSource::Reddit, SourceErrorCategory::InvalidData)
        })?),
        event_fingerprint,
        lineage_id: None,
        payload: EvidencePayload::Reddit(RedditEvidence {
            post_id: candidate.post_id.clone(),
            communities: candidate.cross_community_subreddits.clone(),
            event_type: Some(candidate.event_type.clone()),
            supporting_signals: candidate
                .supporting_signals
                .iter()
                .chain(candidate.context.supporting_signals.iter())
                .cloned()
                .collect(),
            counter_signals: candidate
                .counter_signals
                .iter()
                .chain(candidate.context.counter_signals.iter())
                .cloned()
                .collect(),
            context_comments: Some(candidate.context.comments_scanned),
            event_relevance: relevance,
            mention_velocity: None,
            unique_author_velocity: None,
            engagement_velocity: None,
            sentiment: None,
            sentiment_delta: None,
            sentiment_dispersion: None,
            cross_community_confirmation: Some(decimal_score_value(
                candidate.cross_community_confirmation,
            )?),
        }),
    }))
}

fn parse_timestamp_str(value: &str) -> Result<OffsetDateTime, SourceError> {
    OffsetDateTime::parse(value, &time::format_description::well_known::Rfc3339)
        .map_err(|_| SourceError::new(IntelligenceSource::Reddit, SourceErrorCategory::InvalidData))
}

fn decimal_score_value(value: f64) -> Result<Score, SourceError> {
    let value = rust_decimal::Decimal::from_f64_retain(value).ok_or_else(|| {
        SourceError::new(IntelligenceSource::Reddit, SourceErrorCategory::InvalidData)
    })?;
    Score::new(value)
        .map_err(|_| SourceError::new(IntelligenceSource::Reddit, SourceErrorCategory::InvalidData))
}
