//! Structured ingress for decisions emitted by the independent
//! `prooftrade-agent` Hermes runtime.
//!
//! A [`ValidatedAgentDecision`] is a validated proposed intent. It is not
//! RiskEngine approval and it has no execution capability.

use rust_decimal::Decimal;
use serde::Deserialize;
use thiserror::Error;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::domain::{
    DecisionId, DomainError, EvidenceDirection, EvidenceId, EvidenceReference, Instrument,
    Invalidation, LineageId, SourceEventId, SourceKind, TradeAction, TradeDecision,
    TradingUniverse, Urgency,
};

pub const CURRENT_SCHEMA_VERSION: u16 = 1;
pub const HERMES_PROFILE: &str = "prooftrade-agent";
pub const INVALID_AGENT_DECISION: &str = "INVALID_AGENT_DECISION";
pub const NO_TRADE: &str = "NO_TRADE";

#[derive(Debug, Error)]
pub enum HermesError {
    #[error("malformed Hermes decision payload: {0}")]
    MalformedPayload(#[from] serde_json::Error),
    #[error("unsupported Hermes schema version: {0}")]
    UnsupportedSchemaVersion(u16),
    #[error("unexpected Hermes profile: {0}")]
    UnexpectedProfile(String),
    #[error("missing required Hermes decision field: {0}")]
    MissingField(&'static str),
    #[error("invalid Hermes decision field {field}: {reason}")]
    InvalidField { field: &'static str, reason: String },
    #[error("Hermes decision references an instrument outside the configured universe: {0}")]
    UnknownInstrument(String),
    #[error("Hermes decision failed domain validation: {0}")]
    DomainValidation(#[source] DomainError),
}

impl HermesError {
    /// Every rejected agent payload is explicitly a no-trade outcome.
    pub const fn means_no_trade(&self) -> bool {
        true
    }

    pub const fn code(&self) -> &'static str {
        INVALID_AGENT_DECISION
    }

    pub const fn outcome(&self) -> &'static str {
        NO_TRADE
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawHermesDecision {
    pub schema_version: u16,
    pub profile: String,
    pub request_id: String,
    pub run_id: String,
    #[serde(deserialize_with = "deserialize_timestamp")]
    pub emitted_at: OffsetDateTime,
    pub decision_id: Uuid,
    pub instrument: String,
    pub action: TradeAction,
    pub confidence: Decimal,
    pub support_strength: Decimal,
    pub counter_signal_strength: Decimal,
    #[serde(alias = "expected_horizon_sec")]
    pub expected_horizon_secs: Option<u64>,
    pub desired_exposure_pct: Option<Decimal>,
    pub requested_notional: Option<Decimal>,
    pub desired_reduction_pct: Option<Decimal>,
    pub urgency: Urgency,
    pub max_acceptable_price: Option<Decimal>,
    #[serde(default)]
    pub why_trade: Vec<String>,
    #[serde(default)]
    pub why_not_trade: Vec<String>,
    #[serde(default)]
    pub reconsider_if: Vec<String>,
    #[serde(default)]
    pub invalidation: Option<RawInvalidation>,
    #[serde(default)]
    pub evidence_refs: Vec<RawEvidenceReference>,
    #[serde(deserialize_with = "deserialize_timestamp")]
    pub created_at: OffsetDateTime,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawInvalidation {
    pub description: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawEvidenceReference {
    pub evidence_id: Uuid,
    pub source: SourceKind,
    pub source_event_id: String,
    pub lineage_id: Uuid,
    pub instrument: Option<String>,
    #[serde(deserialize_with = "deserialize_timestamp")]
    pub observed_at: OffsetDateTime,
    pub direction: EvidenceDirection,
    pub strength: Decimal,
    pub freshness: Decimal,
    pub reliability: Decimal,
    pub novelty: Decimal,
    pub relevance: Decimal,
    pub source_reference: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DecisionProvenance {
    pub schema_version: u16,
    pub profile: String,
    pub request_id: String,
    pub run_id: String,
    pub emitted_at: OffsetDateTime,
}

/// A validated Hermes intent paired with provenance.
///
/// This type is deliberately not an approved order. Approval and execution
/// remain owned by later RiskEngine and ExecutionEngine boundaries.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidatedAgentDecision {
    decision: TradeDecision,
    provenance: DecisionProvenance,
}

impl ValidatedAgentDecision {
    pub fn decision(&self) -> &TradeDecision {
        &self.decision
    }

    pub fn provenance(&self) -> &DecisionProvenance {
        &self.provenance
    }
}

/// Request metadata shared by future Hermes transports.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HermesRequest {
    pub request_id: String,
}

/// Narrow transport seam; implementations return structured JSON for the
/// decoder and expose no exchange or order-writing capability.
pub trait HermesClient {
    type Error;

    fn request_decision(&self, request: &HermesRequest) -> Result<String, Self::Error>;
}

pub fn decode_agent_decision(
    payload: &str,
    universe: &TradingUniverse,
) -> Result<ValidatedAgentDecision, HermesError> {
    let raw: RawHermesDecision = serde_json::from_str(payload)?;
    raw.into_validated(universe)
}

pub fn decode_agent_decision_for_request(
    payload: &str,
    universe: &TradingUniverse,
    request: &HermesRequest,
) -> Result<ValidatedAgentDecision, HermesError> {
    let raw: RawHermesDecision = serde_json::from_str(payload)?;
    if raw.request_id != request.request_id {
        return Err(HermesError::InvalidField {
            field: "request_id",
            reason: "does not match the active Hermes request".to_owned(),
        });
    }
    raw.into_validated(universe)
}

impl RawHermesDecision {
    fn into_validated(
        self,
        universe: &TradingUniverse,
    ) -> Result<ValidatedAgentDecision, HermesError> {
        if self.schema_version != CURRENT_SCHEMA_VERSION {
            return Err(HermesError::UnsupportedSchemaVersion(self.schema_version));
        }
        if self.profile != HERMES_PROFILE {
            return Err(HermesError::UnexpectedProfile(self.profile));
        }
        require_non_empty(&self.request_id, "request_id")?;
        require_non_empty(&self.run_id, "run_id")?;
        if self.decision_id.is_nil() {
            return Err(HermesError::InvalidField {
                field: "decision_id",
                reason: "must not be the nil UUID".to_owned(),
            });
        }

        universe.validate().map_err(HermesError::DomainValidation)?;
        let instrument =
            Instrument::parse(&self.instrument).map_err(|_| HermesError::InvalidField {
                field: "instrument",
                reason: "must be a valid spot symbol".to_owned(),
            })?;
        if !universe.contains(&instrument) {
            return Err(HermesError::UnknownInstrument(self.instrument));
        }

        let decision_id = DecisionId::from_uuid(self.decision_id);
        let mut decision = match self.action {
            TradeAction::Buy => TradeDecision::buy(
                decision_id,
                instrument.clone(),
                self.created_at,
                self.requested_notional
                    .ok_or(HermesError::MissingField("requested_notional"))?,
                self.desired_exposure_pct
                    .ok_or(HermesError::MissingField("desired_exposure_pct"))?,
            ),
            TradeAction::Sell => TradeDecision::sell(
                decision_id,
                instrument.clone(),
                self.created_at,
                self.desired_reduction_pct
                    .ok_or(HermesError::MissingField("desired_reduction_pct"))?,
            ),
            TradeAction::Hold => TradeDecision::hold(
                decision_id,
                instrument.clone(),
                self.created_at,
                self.why_not_trade
                    .iter()
                    .find(|reason| !reason.trim().is_empty())
                    .cloned()
                    .unwrap_or_default(),
            ),
        };

        decision.confidence = self.confidence;
        decision.support_strength = self.support_strength;
        decision.counter_signal_strength = self.counter_signal_strength;
        decision.expected_horizon_secs = self.expected_horizon_secs;
        decision.desired_exposure_pct = self.desired_exposure_pct;
        decision.requested_notional = self.requested_notional;
        decision.desired_reduction_pct = self.desired_reduction_pct;
        decision.urgency = self.urgency;
        decision.max_acceptable_price = self.max_acceptable_price;
        decision.why_trade = self.why_trade;
        decision.why_not_trade = self.why_not_trade;
        decision.reconsider_if = self.reconsider_if;
        decision.invalidation = self.invalidation.map(|value| Invalidation {
            description: value.description,
        });
        decision.evidence_refs = self
            .evidence_refs
            .into_iter()
            .map(|reference| convert_evidence_reference(reference, &instrument))
            .collect::<Result<Vec<_>, _>>()?;
        decision.validate().map_err(HermesError::DomainValidation)?;

        Ok(ValidatedAgentDecision {
            decision,
            provenance: DecisionProvenance {
                schema_version: self.schema_version,
                profile: self.profile,
                request_id: self.request_id,
                run_id: self.run_id,
                emitted_at: self.emitted_at,
            },
        })
    }
}

fn require_non_empty(value: &str, field: &'static str) -> Result<(), HermesError> {
    if value.trim().is_empty() {
        return Err(HermesError::MissingField(field));
    }
    Ok(())
}

fn deserialize_timestamp<'de, D>(deserializer: D) -> Result<OffsetDateTime, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    OffsetDateTime::parse(&value, &time::format_description::well_known::Rfc3339)
        .map_err(serde::de::Error::custom)
}

fn convert_evidence_reference(
    raw: RawEvidenceReference,
    decision_instrument: &Instrument,
) -> Result<EvidenceReference, HermesError> {
    if raw.evidence_id.is_nil() {
        return Err(HermesError::InvalidField {
            field: "evidence_refs.evidence_id",
            reason: "must not be the nil UUID".to_owned(),
        });
    }
    if raw.lineage_id.is_nil() {
        return Err(HermesError::InvalidField {
            field: "evidence_refs.lineage_id",
            reason: "must not be the nil UUID".to_owned(),
        });
    }
    let evidence_instrument = raw
        .instrument
        .map(|value| {
            Instrument::parse(&value).map_err(|_| HermesError::InvalidField {
                field: "evidence_refs.instrument",
                reason: "must be a valid spot symbol".to_owned(),
            })
        })
        .transpose()?;
    let reference = EvidenceReference {
        evidence_id: EvidenceId::from_uuid(raw.evidence_id),
        source: raw.source,
        source_event_id: SourceEventId::new(raw.source_event_id)
            .map_err(HermesError::DomainValidation)?,
        lineage_id: LineageId::from_uuid(raw.lineage_id),
        instrument: evidence_instrument,
        observed_at: raw.observed_at,
        direction: raw.direction,
        strength: raw.strength,
        freshness: raw.freshness,
        reliability: raw.reliability,
        novelty: raw.novelty,
        relevance: raw.relevance,
        source_reference: raw.source_reference,
    };
    reference
        .validate()
        .map_err(HermesError::DomainValidation)?;
    if let Some(instrument) = &reference.instrument {
        if instrument != decision_instrument {
            return Err(HermesError::DomainValidation(
                DomainError::InstrumentMismatch,
            ));
        }
    }
    Ok(reference)
}
