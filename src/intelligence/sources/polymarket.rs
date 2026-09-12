use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::env;
use std::path::PathBuf;
use std::process::Command;
use std::str::FromStr;
use std::time::Duration;

use rust_decimal::Decimal;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use thiserror::Error;
use time::OffsetDateTime;
use uuid::Uuid;

use super::{IntelligenceSourceAdapter, SourceError};
use crate::atk::{McpError, McpTransport, StdioTransport};
use crate::domain::{EvidenceId, Instrument, SourceEventId};
use crate::intelligence::{
    AssetImpact, BoundedEvidenceQuery, EventIdentity, EvidencePayload, IntelligenceDirection,
    IntelligenceError, IntelligenceEvidence, IntelligenceSource, PredictionMarketEvidence,
    Probability, Score, SignedValue, SourceErrorCategory, SourceReference,
};

#[derive(Clone, Debug)]
struct ProbabilityObservation {
    observed_at: OffsetDateTime,
    probability: Probability,
}

#[derive(Clone, Debug)]
pub struct ProbabilityHistory {
    capacity: usize,
    observations: HashMap<String, VecDeque<ProbabilityObservation>>,
}

impl ProbabilityHistory {
    pub fn new(capacity: usize) -> Result<Self, IntelligenceError> {
        if capacity == 0 {
            return Err(IntelligenceError::InvalidHistoryCapacity);
        }
        Ok(Self {
            capacity,
            observations: HashMap::new(),
        })
    }

    pub fn record(
        &mut self,
        market_id: &str,
        observed_at: OffsetDateTime,
        probability: Probability,
    ) -> Result<Option<SignedValue>, IntelligenceError> {
        if market_id.trim().is_empty() {
            return Err(IntelligenceError::EmptyField { field: "market_id" });
        }

        let history = self.observations.entry(market_id.to_owned()).or_default();
        let delta = if let Some(previous) = history.back() {
            if observed_at <= previous.observed_at {
                return Err(IntelligenceError::OutOfOrderObservation);
            }
            Some(SignedValue::new(
                probability.value() - previous.probability.value(),
            )?)
        } else {
            None
        };

        history.push_back(ProbabilityObservation {
            observed_at,
            probability,
        });
        while history.len() > self.capacity {
            history.pop_front();
        }

        Ok(delta)
    }

    pub fn len_for(&self, market_id: &str) -> usize {
        self.observations.get(market_id).map_or(0, VecDeque::len)
    }

    pub fn latest_probability(&self, market_id: &str) -> Option<Decimal> {
        self.observations
            .get(market_id)
            .and_then(VecDeque::back)
            .map(|observation| observation.probability.value())
    }
}

#[derive(Clone, Debug)]
pub struct PolymarketConfig {
    enabled: bool,
    executable: PathBuf,
    gamma_api_url: String,
    clob_api_url: String,
    request_timeout: Duration,
    history_capacity: usize,
    max_markets_per_term: usize,
    search_terms: BTreeMap<Instrument, Vec<String>>,
}

impl Default for PolymarketConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            executable: PathBuf::from("polymarket-mcp"),
            gamma_api_url: "https://gamma-api.polymarket.com".to_owned(),
            clob_api_url: "https://clob.polymarket.com".to_owned(),
            request_timeout: Duration::from_secs(15),
            history_capacity: 32,
            max_markets_per_term: 5,
            search_terms: default_search_terms(),
        }
    }
}

impl PolymarketConfig {
    pub fn disabled() -> Self {
        Self::default().with_enabled(false)
    }

    pub fn from_env() -> Result<Self, PolymarketMcpError> {
        let mut config = Self::default();
        if let Some(value) = env::var_os("PROOFTRADE_POLYMARKET_MCP_BIN") {
            config.executable = PathBuf::from(value);
        }
        if let Ok(value) = env::var("PROOFTRADE_POLYMARKET_GAMMA_URL") {
            config.gamma_api_url = value;
        }
        if let Ok(value) = env::var("PROOFTRADE_POLYMARKET_CLOB_URL") {
            config.clob_api_url = value;
        }
        if let Ok(value) = env::var("PROOFTRADE_POLYMARKET_ENABLED") {
            config.enabled = parse_bool(&value)?;
        }
        config.validate()?;
        Ok(config)
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn with_enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    pub fn with_executable(mut self, executable: impl Into<PathBuf>) -> Self {
        self.executable = executable.into();
        self
    }

    pub fn with_gamma_api_url(mut self, gamma_api_url: impl Into<String>) -> Self {
        self.gamma_api_url = gamma_api_url.into();
        self
    }

    pub fn with_clob_api_url(mut self, clob_api_url: impl Into<String>) -> Self {
        self.clob_api_url = clob_api_url.into();
        self
    }

    pub fn with_request_timeout(mut self, request_timeout: Duration) -> Self {
        self.request_timeout = request_timeout;
        self
    }

    pub fn with_history_capacity(mut self, history_capacity: usize) -> Self {
        self.history_capacity = history_capacity;
        self
    }

    pub fn with_max_markets_per_term(mut self, max_markets_per_term: usize) -> Self {
        self.max_markets_per_term = max_markets_per_term;
        self
    }

    pub fn with_search_terms(
        mut self,
        instrument: Instrument,
        terms: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.search_terms.insert(
            instrument,
            terms
                .into_iter()
                .map(Into::into)
                .filter(|term| !term.trim().is_empty())
                .collect(),
        );
        self
    }

    fn validate(&self) -> Result<(), PolymarketMcpError> {
        if self.executable.as_os_str().is_empty() {
            return Err(PolymarketMcpError::InvalidConfig(
                "executable cannot be empty".to_owned(),
            ));
        }
        if self.gamma_api_url.trim().is_empty() || self.clob_api_url.trim().is_empty() {
            return Err(PolymarketMcpError::InvalidConfig(
                "Gamma and CLOB API URLs cannot be empty".to_owned(),
            ));
        }
        if self.request_timeout.is_zero() {
            return Err(PolymarketMcpError::InvalidConfig(
                "request timeout must be positive".to_owned(),
            ));
        }
        if self.history_capacity == 0 || self.max_markets_per_term == 0 {
            return Err(PolymarketMcpError::InvalidConfig(
                "history capacity and market limit must be positive".to_owned(),
            ));
        }
        if self.search_terms.is_empty()
            || self.search_terms.iter().any(|(instrument, terms)| {
                instrument.validate().is_err()
                    || terms.is_empty()
                    || terms.iter().any(|term| term.trim().is_empty())
            })
        {
            return Err(PolymarketMcpError::InvalidConfig(
                "at least one valid instrument search term is required".to_owned(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum PolymarketMcpError {
    #[error("invalid Polymarket MCP configuration: {0}")]
    InvalidConfig(String),
    #[error("Polymarket MCP transport error: {0}")]
    Transport(#[from] McpError),
    #[error("Polymarket MCP protocol error: {0}")]
    Protocol(String),
    #[error("invalid Polymarket MCP data: {0}")]
    InvalidData(String),
    #[error("invalid normalized intelligence data: {0}")]
    Intelligence(#[from] IntelligenceError),
    #[error("Polymarket REST error: {0}")]
    Rest(#[from] PolymarketRestError),
}

impl PolymarketMcpError {
    fn source_category(&self) -> SourceErrorCategory {
        match self {
            Self::Transport(McpError::RequestTimeout { .. }) => SourceErrorCategory::Timeout,
            Self::Transport(McpError::ProcessExited | McpError::Io(_)) => {
                SourceErrorCategory::ProcessFailure
            }
            Self::Transport(McpError::MalformedJson(_))
            | Self::Transport(McpError::InvalidEnvelope(_))
            | Self::Protocol(_)
            | Self::InvalidData(_)
            | Self::Intelligence(_)
            | Self::InvalidConfig(_) => SourceErrorCategory::MalformedResponse,
            Self::Transport(McpError::JsonRpc { .. }) => SourceErrorCategory::Unavailable,
            Self::Rest(error) => error.source_category(),
        }
    }
}

pub trait PolymarketReader {
    fn collect_polymarket(
        &mut self,
        query: &BoundedEvidenceQuery,
    ) -> Result<Vec<IntelligenceEvidence>, SourceError>;
}

pub struct PolymarketMcpReader<T> {
    transport: T,
    config: PolymarketConfig,
    history: ProbabilityHistory,
}

impl<T: McpTransport> PolymarketMcpReader<T> {
    pub fn from_transport(
        transport: T,
        config: PolymarketConfig,
    ) -> Result<Self, PolymarketMcpError> {
        config.validate()?;
        if !transport.is_alive() {
            return Err(PolymarketMcpError::Transport(McpError::ProcessExited));
        }
        let initialize = transport.request(
            "initialize",
            json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {"name": "prooftrade", "version": "0.1.0"}
            }),
            config.request_timeout,
        )?;
        let initialize = rpc_result(&initialize)?;
        if initialize
            .get("protocolVersion")
            .and_then(Value::as_str)
            .is_none_or(|version| version.trim().is_empty())
        {
            return Err(PolymarketMcpError::Protocol(
                "initialize.protocolVersion is missing".to_owned(),
            ));
        }
        let tools = transport.request("tools/list", json!({}), config.request_timeout)?;
        let tools = rpc_result(&tools)?;
        let has_search = tools
            .get("tools")
            .and_then(Value::as_array)
            .is_some_and(|items| {
                items
                    .iter()
                    .any(|item| item.get("name").and_then(Value::as_str) == Some("search_markets"))
            });
        if !has_search {
            return Err(PolymarketMcpError::Protocol(
                "required search_markets tool is missing".to_owned(),
            ));
        }
        transport.notify("notifications/initialized", json!({}))?;
        let history = ProbabilityHistory::new(config.history_capacity)?;
        Ok(Self {
            transport,
            config,
            history,
        })
    }

    pub fn is_alive(&self) -> bool {
        self.transport.is_alive()
    }

    pub fn shutdown(&self) -> Result<(), McpError> {
        self.transport.shutdown()
    }

    fn collect_inner(
        &mut self,
        query: &BoundedEvidenceQuery,
    ) -> Result<Vec<IntelligenceEvidence>, PolymarketMcpError> {
        let configured_terms = self.config.search_terms.clone();
        let mut seen = BTreeSet::new();
        let mut evidence = Vec::new();
        for (instrument, terms) in configured_terms {
            if !query.instruments().is_empty() && !query.instruments().contains(&instrument) {
                continue;
            }
            let limit = self.config.max_markets_per_term.min(query.max_per_source()) as u64;
            for term in terms {
                let response = self.transport.request(
                    "tools/call",
                    json!({
                        "name": "search_markets",
                        "arguments": {"keyword": term, "limit": limit}
                    }),
                    self.config.request_timeout,
                )?;
                let payload = tool_payload(&response)?;
                let Some(markets) = payload.get("markets").and_then(Value::as_array) else {
                    return Err(PolymarketMcpError::InvalidData(
                        "search_markets response is missing markets".to_owned(),
                    ));
                };
                for market in markets {
                    if evidence.len() >= query.max_results() {
                        break;
                    }
                    let Some(item) = self.market_to_evidence(market, &instrument)? else {
                        continue;
                    };
                    let key = (
                        item.source_event_id.as_str().to_owned(),
                        item.affected_assets[0].instrument.clone(),
                    );
                    if seen.insert(key) {
                        evidence.push(item);
                    }
                }
            }
        }
        Ok(evidence)
    }

    fn market_to_evidence(
        &mut self,
        market: &Value,
        instrument: &Instrument,
    ) -> Result<Option<IntelligenceEvidence>, PolymarketMcpError> {
        if market.get("active").and_then(Value::as_bool) == Some(false)
            || market.get("closed").and_then(Value::as_bool) == Some(true)
        {
            return Ok(None);
        }
        let Some(market_id) = non_empty_string(market.get("id")) else {
            return Ok(None);
        };
        let Some(question) = non_empty_string(market.get("question")) else {
            return Ok(None);
        };
        let outcomes = string_array(market.get("outcomes"));
        let prices = string_array(market.get("outcomePrices"));
        let token_ids = string_array(market.get("clobTokenIds"));
        if prices.is_empty() || token_ids.is_empty() {
            return Ok(None);
        }
        let outcome_index = outcomes
            .iter()
            .position(|outcome| outcome.eq_ignore_ascii_case("yes"))
            .unwrap_or(0);
        let (Some(price_value), Some(token_id)) =
            (prices.get(outcome_index), token_ids.get(outcome_index))
        else {
            return Ok(None);
        };
        let probability = Probability::new(parse_decimal(price_value, "outcome price")?)?;
        let observed_at = OffsetDateTime::now_utc();
        let history_key = format!("{market_id}:{token_id}");
        let probability_delta = self
            .history
            .record(&history_key, observed_at, probability)?;
        let event_fingerprint = EventIdentity::new(
            "prediction_market",
            &question,
            vec![market_id.clone()],
            vec![instrument.clone()],
            None,
            3_600,
        )
        .and_then(|identity| identity.fingerprint())?;
        let source_event_id = SourceEventId::new(history_key.clone())
            .map_err(|error| PolymarketMcpError::InvalidData(error.to_string()))?;
        let source_reference = market
            .get("slug")
            .and_then(Value::as_str)
            .filter(|slug| !slug.trim().is_empty())
            .map(|slug| format!("https://polymarket.com/event/{slug}"))
            .unwrap_or_else(|| format!("polymarket://market/{market_id}"));
        Ok(Some(IntelligenceEvidence {
            evidence_id: stable_evidence_id(&history_key),
            source: IntelligenceSource::Polymarket,
            source_event_id,
            event_time: Some(observed_at),
            observed_at,
            affected_assets: vec![AssetImpact {
                instrument: instrument.clone(),
                relevance: Score::new(Decimal::ONE)?,
            }],
            direction: IntelligenceDirection::Unclear,
            strength: Some(Score::new(probability.value())?),
            freshness: Some(Score::new(Decimal::ONE)?),
            reliability: Some(Score::new(Decimal::new(50, 2))?),
            novelty: Some(Score::new(
                probability_delta
                    .map(|delta| delta.value().abs())
                    .unwrap_or(Decimal::ZERO),
            )?),
            summary: format!(
                "Polymarket MCP probability for {question}: {}",
                probability.value()
            ),
            source_reference: Some(SourceReference::new(source_reference)?),
            event_fingerprint,
            lineage_id: None,
            payload: EvidencePayload::PredictionMarket(PredictionMarketEvidence {
                market_id,
                token_id: token_id.clone(),
                question,
                implied_probability: probability,
                probability_delta,
                liquidity: parse_optional_decimal(market.get("liquidity")),
                volume: parse_optional_decimal(market.get("volume")),
                spread: None,
            }),
        }))
    }
}

pub struct PolymarketAdapter<R> {
    reader: R,
    enabled: bool,
}

impl<R> PolymarketAdapter<R> {
    pub fn new(reader: R) -> Self {
        Self::with_enabled(reader, true)
    }

    pub fn with_enabled(reader: R, enabled: bool) -> Self {
        Self { reader, enabled }
    }
}

impl<R: PolymarketReader> IntelligenceSourceAdapter for PolymarketAdapter<R> {
    fn source(&self) -> IntelligenceSource {
        IntelligenceSource::Polymarket
    }

    fn is_enabled(&self) -> bool {
        self.enabled
    }

    fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    fn collect(
        &mut self,
        query: &BoundedEvidenceQuery,
    ) -> Result<Vec<IntelligenceEvidence>, SourceError> {
        if !self.enabled {
            return Err(SourceError::new(
                IntelligenceSource::Polymarket,
                SourceErrorCategory::Disabled,
            ));
        }
        self.reader.collect_polymarket(query)
    }
}

impl PolymarketMcpReader<StdioTransport> {
    pub fn launch(config: PolymarketConfig) -> Result<Self, PolymarketMcpError> {
        config.validate()?;
        let mut command = Command::new(&config.executable);
        let transport = StdioTransport::spawn(&mut command)?;
        Self::from_transport(transport, config)
    }
}

impl<T: McpTransport> PolymarketReader for PolymarketMcpReader<T> {
    fn collect_polymarket(
        &mut self,
        query: &BoundedEvidenceQuery,
    ) -> Result<Vec<IntelligenceEvidence>, SourceError> {
        self.collect_inner(query).map_err(|error| {
            SourceError::new(IntelligenceSource::Polymarket, error.source_category())
        })
    }
}

/// Transport seam for the public Polymarket Gamma and CLOB REST APIs.
///
/// Keeping this seam separate from the MCP transport makes REST parsing and
/// fallback behavior deterministic in tests while the production
/// implementation remains a read-only HTTP client.
pub trait PolymarketRestTransport {
    fn get_json(
        &mut self,
        url: &str,
        query: &BTreeMap<String, String>,
    ) -> Result<Value, PolymarketRestError>;

    fn post_json(&mut self, url: &str, body: Value) -> Result<Value, PolymarketRestError>;
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum PolymarketRestError {
    #[error("Polymarket REST request failed: {0}")]
    Request(String),
    #[error("Polymarket REST request timed out: {0}")]
    Timeout(String),
    #[error("Polymarket REST returned HTTP {status}: {body}")]
    HttpStatus { status: u16, body: String },
    #[error("invalid Polymarket REST data: {0}")]
    InvalidData(String),
}

impl PolymarketRestError {
    fn source_category(&self) -> SourceErrorCategory {
        match self {
            Self::HttpStatus { status: 429, .. } => SourceErrorCategory::RateLimited,
            Self::HttpStatus {
                status: 401 | 403, ..
            } => SourceErrorCategory::Unauthorized,
            Self::HttpStatus { status: 408, .. } | Self::Timeout(_) => SourceErrorCategory::Timeout,
            Self::Request(_) => SourceErrorCategory::Unavailable,
            Self::HttpStatus {
                status: 500..=599, ..
            } => SourceErrorCategory::Unavailable,
            Self::HttpStatus { .. } => SourceErrorCategory::MalformedResponse,
            Self::InvalidData(_) => SourceErrorCategory::MalformedResponse,
        }
    }
}

pub struct ReqwestPolymarketRestTransport {
    client: reqwest::blocking::Client,
}

impl ReqwestPolymarketRestTransport {
    pub fn new(timeout: Duration) -> Result<Self, PolymarketRestError> {
        let client = reqwest::blocking::Client::builder()
            .timeout(timeout)
            .user_agent("prooftrade/0.1 polymarket-read-only")
            .build()
            .map_err(|error| PolymarketRestError::Request(error.to_string()))?;
        Ok(Self { client })
    }

    fn decode_response(
        response: reqwest::blocking::Response,
    ) -> Result<Value, PolymarketRestError> {
        let status = response.status();
        let body = response
            .text()
            .map_err(|error| PolymarketRestError::Request(error.to_string()))?;
        if !status.is_success() {
            return Err(PolymarketRestError::HttpStatus {
                status: status.as_u16(),
                body: truncate_error_body(&body),
            });
        }
        serde_json::from_str(&body)
            .map_err(|error| PolymarketRestError::InvalidData(error.to_string()))
    }
}

impl PolymarketRestTransport for ReqwestPolymarketRestTransport {
    fn get_json(
        &mut self,
        url: &str,
        query: &BTreeMap<String, String>,
    ) -> Result<Value, PolymarketRestError> {
        let response = self
            .client
            .get(url)
            .query(query)
            .send()
            .map_err(rest_request_error)?;
        Self::decode_response(response)
    }

    fn post_json(&mut self, url: &str, body: Value) -> Result<Value, PolymarketRestError> {
        let response = self
            .client
            .post(url)
            .json(&body)
            .send()
            .map_err(rest_request_error)?;
        Self::decode_response(response)
    }
}

pub struct PolymarketRestReader<T> {
    transport: T,
    config: PolymarketConfig,
    history: ProbabilityHistory,
}

impl<T: PolymarketRestTransport> PolymarketRestReader<T> {
    pub fn from_transport(
        transport: T,
        config: PolymarketConfig,
    ) -> Result<Self, PolymarketRestError> {
        config
            .validate()
            .map_err(|error| PolymarketRestError::InvalidData(error.to_string()))?;
        let history = ProbabilityHistory::new(config.history_capacity)
            .map_err(|error| PolymarketRestError::InvalidData(error.to_string()))?;
        Ok(Self {
            transport,
            config,
            history,
        })
    }

    fn collect_inner(
        &mut self,
        query: &BoundedEvidenceQuery,
    ) -> Result<Vec<IntelligenceEvidence>, PolymarketRestError> {
        let configured_terms = self.config.search_terms.clone();
        let mut candidates = Vec::new();
        let mut seen_markets = BTreeSet::new();
        for (instrument, terms) in configured_terms {
            if !query.instruments().is_empty() && !query.instruments().contains(&instrument) {
                continue;
            }
            for term in terms {
                let response = self.transport.get_json(
                    &join_url(&self.config.gamma_api_url, "public-search"),
                    &BTreeMap::from([
                        ("q".to_owned(), term),
                        (
                            "limit_per_type".to_owned(),
                            self.config
                                .max_markets_per_term
                                .min(query.max_per_source())
                                .to_string(),
                        ),
                        ("search_profiles".to_owned(), "false".to_owned()),
                        ("search_tags".to_owned(), "false".to_owned()),
                        ("keep_closed_markets".to_owned(), "0".to_owned()),
                    ]),
                )?;
                let mut markets = Vec::new();
                collect_market_values(&response, &mut markets);
                for market in markets {
                    let Some(candidate) = self.market_candidate(market, &instrument)? else {
                        continue;
                    };
                    if seen_markets.insert((candidate.market_id.clone(), instrument.clone())) {
                        candidates.push(candidate);
                    }
                    if candidates.len() >= query.max_results() {
                        break;
                    }
                }
                if candidates.len() >= query.max_results() {
                    break;
                }
            }
            if candidates.len() >= query.max_results() {
                break;
            }
        }

        if candidates.is_empty() {
            return Ok(Vec::new());
        }
        let token_requests = candidates
            .iter()
            .map(|candidate| json!({"token_id": candidate.token_id}))
            .collect::<Vec<_>>();
        let midpoint = self.transport.post_json(
            &join_url(&self.config.clob_api_url, "midpoints"),
            Value::Array(token_requests.clone()),
        )?;
        let spreads = self.transport.post_json(
            &join_url(&self.config.clob_api_url, "spreads"),
            Value::Array(token_requests),
        )?;

        candidates
            .into_iter()
            .filter_map(|candidate| {
                self.candidate_to_evidence(&candidate, &midpoint, &spreads)
                    .transpose()
            })
            .collect()
    }

    fn market_candidate(
        &self,
        market: Value,
        instrument: &Instrument,
    ) -> Result<Option<RestMarketCandidate>, PolymarketRestError> {
        if market.get("active").and_then(Value::as_bool) == Some(false)
            || market.get("closed").and_then(Value::as_bool) == Some(true)
            || market.get("archived").and_then(Value::as_bool) == Some(true)
        {
            return Ok(None);
        }
        let Some(market_id) = non_empty_string(market.get("id")) else {
            return Ok(None);
        };
        let Some(question) = non_empty_string(market.get("question")) else {
            return Ok(None);
        };
        let outcomes = string_array(market.get("outcomes"));
        let token_ids = string_array(market.get("clobTokenIds"));
        if token_ids.is_empty() {
            return Ok(None);
        }
        let outcome_index = outcomes
            .iter()
            .position(|outcome| outcome.eq_ignore_ascii_case("yes"))
            .unwrap_or(0);
        let Some(token_id) = token_ids
            .get(outcome_index)
            .filter(|value| !value.is_empty())
        else {
            return Ok(None);
        };
        Ok(Some(RestMarketCandidate {
            market,
            market_id,
            question,
            token_id: token_id.clone(),
            instrument: instrument.clone(),
        }))
    }

    fn candidate_to_evidence(
        &mut self,
        candidate: &RestMarketCandidate,
        midpoint: &Value,
        spreads: &Value,
    ) -> Result<Option<IntelligenceEvidence>, PolymarketRestError> {
        let Some(midpoint) = metric_for_token(midpoint, &candidate.token_id) else {
            return Ok(None);
        };
        let probability = Probability::new(
            parse_decimal(&midpoint, "CLOB midpoint")
                .map_err(|error| PolymarketRestError::InvalidData(error.to_string()))?,
        )
        .map_err(|error| PolymarketRestError::InvalidData(error.to_string()))?;
        let observed_at = OffsetDateTime::now_utc();
        let probability_delta = self
            .history
            .record(
                &format!("{}:{}", candidate.market_id, candidate.token_id),
                observed_at,
                probability,
            )
            .map_err(|error| PolymarketRestError::InvalidData(error.to_string()))?;
        let event_fingerprint = EventIdentity::new(
            "prediction_market",
            &candidate.question,
            vec![candidate.market_id.clone()],
            vec![candidate.instrument.clone()],
            None,
            3_600,
        )
        .and_then(|identity| identity.fingerprint())
        .map_err(|error| PolymarketRestError::InvalidData(error.to_string()))?;
        let source_event_id =
            SourceEventId::new(format!("{}:{}", candidate.market_id, candidate.token_id))
                .map_err(|error| PolymarketRestError::InvalidData(error.to_string()))?;
        let source_reference = candidate
            .market
            .get("slug")
            .and_then(Value::as_str)
            .filter(|slug| !slug.trim().is_empty())
            .map(|slug| format!("https://polymarket.com/event/{slug}"))
            .unwrap_or_else(|| format!("polymarket://market/{}", candidate.market_id));
        let key = format!("{}:{}", candidate.market_id, candidate.token_id);
        let spread = metric_for_token(spreads, &candidate.token_id)
            .map(|value| {
                parse_decimal(&value, "CLOB spread")
                    .map_err(|error| PolymarketRestError::InvalidData(error.to_string()))
            })
            .transpose()?;
        Ok(Some(IntelligenceEvidence {
            evidence_id: stable_evidence_id(&key),
            source: IntelligenceSource::Polymarket,
            source_event_id,
            event_time: Some(observed_at),
            observed_at,
            affected_assets: vec![AssetImpact {
                instrument: candidate.instrument.clone(),
                relevance: Score::new(Decimal::ONE)
                    .map_err(|error| PolymarketRestError::InvalidData(error.to_string()))?,
            }],
            direction: IntelligenceDirection::Unclear,
            strength: Some(
                Score::new(probability.value())
                    .map_err(|error| PolymarketRestError::InvalidData(error.to_string()))?,
            ),
            freshness: Some(
                Score::new(Decimal::ONE)
                    .map_err(|error| PolymarketRestError::InvalidData(error.to_string()))?,
            ),
            reliability: Some(
                Score::new(Decimal::new(65, 2))
                    .map_err(|error| PolymarketRestError::InvalidData(error.to_string()))?,
            ),
            novelty: Some(
                Score::new(
                    probability_delta
                        .map(|delta| delta.value().abs())
                        .unwrap_or(Decimal::ZERO),
                )
                .map_err(|error| PolymarketRestError::InvalidData(error.to_string()))?,
            ),
            summary: format!(
                "Polymarket CLOB midpoint for {}: {}",
                candidate.question,
                probability.value()
            ),
            source_reference: Some(
                SourceReference::new(source_reference)
                    .map_err(|error| PolymarketRestError::InvalidData(error.to_string()))?,
            ),
            event_fingerprint,
            lineage_id: None,
            payload: EvidencePayload::PredictionMarket(PredictionMarketEvidence {
                market_id: candidate.market_id.clone(),
                token_id: candidate.token_id.clone(),
                question: candidate.question.clone(),
                implied_probability: probability,
                probability_delta,
                liquidity: parse_optional_decimal(candidate.market.get("liquidity")),
                volume: parse_optional_decimal(candidate.market.get("volume")),
                spread,
            }),
        }))
    }
}

impl PolymarketRestReader<ReqwestPolymarketRestTransport> {
    pub fn launch(config: PolymarketConfig) -> Result<Self, PolymarketRestError> {
        let transport = ReqwestPolymarketRestTransport::new(config.request_timeout)?;
        Self::from_transport(transport, config)
    }
}

impl<T: PolymarketRestTransport> PolymarketReader for PolymarketRestReader<T> {
    fn collect_polymarket(
        &mut self,
        query: &BoundedEvidenceQuery,
    ) -> Result<Vec<IntelligenceEvidence>, SourceError> {
        self.collect_inner(query).map_err(|error| {
            SourceError::new(IntelligenceSource::Polymarket, error.source_category())
        })
    }
}

pub struct PolymarketHybridReader<M, R> {
    mcp: Option<M>,
    rest: Option<R>,
}

impl<M, R> PolymarketHybridReader<M, R> {
    pub fn new(mcp: Option<M>, rest: Option<R>) -> Result<Self, PolymarketMcpError> {
        if mcp.is_none() && rest.is_none() {
            return Err(PolymarketMcpError::InvalidConfig(
                "at least one Polymarket reader is required".to_owned(),
            ));
        }
        Ok(Self { mcp, rest })
    }
}

impl<M: PolymarketReader, R: PolymarketReader> PolymarketReader for PolymarketHybridReader<M, R> {
    fn collect_polymarket(
        &mut self,
        query: &BoundedEvidenceQuery,
    ) -> Result<Vec<IntelligenceEvidence>, SourceError> {
        let mut errors = Vec::new();
        let mut merged = BTreeMap::new();
        if let Some(mcp) = &mut self.mcp {
            match mcp.collect_polymarket(query) {
                Ok(evidence) => {
                    for item in evidence {
                        merged.insert(item.source_event_id.as_str().to_owned(), item);
                    }
                }
                Err(error) => errors.push(error),
            }
        }
        if let Some(rest) = &mut self.rest {
            match rest.collect_polymarket(query) {
                Ok(evidence) => {
                    // REST is the deterministic live-data path, so it wins
                    // when the same market/token was also found by MCP.
                    for item in evidence {
                        merged.insert(item.source_event_id.as_str().to_owned(), item);
                    }
                }
                Err(error) => errors.push(error),
            }
        }
        if merged.is_empty() && !errors.is_empty() {
            return Err(errors.remove(0));
        }
        Ok(merged.into_values().collect())
    }
}

#[derive(Clone, Debug)]
struct RestMarketCandidate {
    market: Value,
    market_id: String,
    question: String,
    token_id: String,
    instrument: Instrument,
}

fn join_url(base: &str, path: &str) -> String {
    format!(
        "{}/{}",
        base.trim_end_matches('/'),
        path.trim_start_matches('/')
    )
}

fn collect_market_values(value: &Value, markets: &mut Vec<Value>) {
    match value {
        Value::Array(values) => {
            for value in values {
                collect_market_values(value, markets);
            }
        }
        Value::Object(object) => {
            if object.contains_key("id") && object.contains_key("question") {
                markets.push(Value::Object(object.clone()));
            }
            for key in ["markets", "events", "data"] {
                if let Some(value) = object.get(key) {
                    collect_market_values(value, markets);
                }
            }
        }
        _ => {}
    }
}

fn parse_metric(value: &Value) -> Option<String> {
    match value {
        Value::String(value) => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        _ => None,
    }
}

fn metric_for_token(value: &Value, token_id: &str) -> Option<String> {
    value.get(token_id).and_then(parse_metric)
}

fn truncate_error_body(body: &str) -> String {
    const MAX_ERROR_BODY_BYTES: usize = 512;
    if body.len() <= MAX_ERROR_BODY_BYTES {
        body.to_owned()
    } else {
        let end = body
            .char_indices()
            .nth(MAX_ERROR_BODY_BYTES)
            .map_or(body.len(), |(index, _)| index);
        format!("{}...", &body[..end])
    }
}

fn rest_request_error(error: reqwest::Error) -> PolymarketRestError {
    if error.is_timeout() {
        PolymarketRestError::Timeout(error.to_string())
    } else {
        PolymarketRestError::Request(error.to_string())
    }
}

fn default_search_terms() -> BTreeMap<Instrument, Vec<String>> {
    BTreeMap::from([
        (
            instrument("BTC-USDT"),
            vec!["bitcoin".to_owned(), "btc".to_owned()],
        ),
        (
            instrument("ETH-USDT"),
            vec!["ethereum".to_owned(), "eth".to_owned()],
        ),
        (
            instrument("SOL-USDT"),
            vec!["solana".to_owned(), "sol".to_owned()],
        ),
        (
            instrument("HYPE-USDT"),
            vec!["hyperliquid".to_owned(), "hype".to_owned()],
        ),
    ])
}

fn instrument(symbol: &str) -> Instrument {
    Instrument::parse(symbol).expect("default Polymarket instrument must be valid")
}

fn parse_bool(value: &str) -> Result<bool, PolymarketMcpError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" => Ok(false),
        _ => Err(PolymarketMcpError::InvalidConfig(format!(
            "invalid boolean value: {value}"
        ))),
    }
}

fn rpc_result(response: &Value) -> Result<&Value, PolymarketMcpError> {
    if let Some(error) = response.get("error") {
        return Err(PolymarketMcpError::Protocol(error.to_string()));
    }
    response
        .get("result")
        .ok_or_else(|| PolymarketMcpError::Protocol("MCP result is missing".to_owned()))
}

fn tool_payload(response: &Value) -> Result<Value, PolymarketMcpError> {
    let result = rpc_result(response)?;
    if let Some(structured) = result.get("structuredContent") {
        return Ok(structured.clone());
    }
    let text = result
        .get("content")
        .and_then(Value::as_array)
        .and_then(|content| {
            content.iter().find_map(|item| {
                (item.get("type").and_then(Value::as_str) == Some("text"))
                    .then(|| item.get("text").and_then(Value::as_str))
                    .flatten()
            })
        })
        .ok_or_else(|| {
            PolymarketMcpError::Protocol("MCP tool text content is missing".to_owned())
        })?;
    serde_json::from_str(text).map_err(|error| PolymarketMcpError::InvalidData(error.to_string()))
}

fn non_empty_string(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn string_array(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::Array(values)) => values
            .iter()
            .filter_map(|value| non_empty_string(Some(value)))
            .collect(),
        Some(Value::String(value)) => serde_json::from_str::<Value>(value)
            .ok()
            .and_then(|parsed| match parsed {
                Value::Array(values) => Some(
                    values
                        .iter()
                        .filter_map(|value| non_empty_string(Some(value)))
                        .collect(),
                ),
                _ => None,
            })
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

fn parse_decimal(value: &str, field: &str) -> Result<Decimal, PolymarketMcpError> {
    Decimal::from_str(value.trim())
        .map_err(|_| PolymarketMcpError::InvalidData(format!("{field} is not a decimal: {value}")))
}

fn parse_optional_decimal(value: Option<&Value>) -> Option<Decimal> {
    match value {
        Some(Value::String(value)) => parse_decimal(value, "market metric").ok(),
        Some(Value::Number(value)) => parse_decimal(&value.to_string(), "market metric").ok(),
        _ => None,
    }
}

fn stable_evidence_id(key: &str) -> EvidenceId {
    let digest = Sha256::digest(key.as_bytes());
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    EvidenceId::from_uuid(Uuid::from_bytes(bytes))
}
