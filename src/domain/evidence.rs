use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use super::{DomainError, EvidenceId, Instrument, LineageId};

#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    AtkNews,
    Polymarket,
    Reddit,
    MarxFinance,
    OkxMarket,
    Other(String),
}

pub trait EvidenceSource {
    fn source_kind(&self) -> SourceKind;
}

impl EvidenceSource for SourceKind {
    fn source_kind(&self) -> SourceKind {
        self.clone()
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceDirection {
    Supporting,
    Counter,
    Neutral,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SourceEventId(String);

impl SourceEventId {
    pub fn new(value: impl Into<String>) -> Result<Self, DomainError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(DomainError::EmptyIdentifier("source_event_id"));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct EvidenceReference {
    pub evidence_id: EvidenceId,
    pub source: SourceKind,
    pub source_event_id: SourceEventId,
    pub lineage_id: LineageId,
    pub instrument: Option<Instrument>,
    pub observed_at: OffsetDateTime,
    pub direction: EvidenceDirection,
    pub strength: Decimal,
    pub freshness: Decimal,
    pub reliability: Decimal,
    pub novelty: Decimal,
    pub relevance: Decimal,
    pub source_reference: Option<String>,
}

impl EvidenceReference {
    pub fn new(
        evidence_id: EvidenceId,
        source: SourceKind,
        source_event_id: SourceEventId,
        lineage_id: LineageId,
        observed_at: OffsetDateTime,
    ) -> Result<Self, DomainError> {
        let reference = Self {
            evidence_id,
            source,
            source_event_id,
            lineage_id,
            instrument: None,
            observed_at,
            direction: EvidenceDirection::Neutral,
            strength: Decimal::ZERO,
            freshness: Decimal::ZERO,
            reliability: Decimal::ZERO,
            novelty: Decimal::ZERO,
            relevance: Decimal::ZERO,
            source_reference: None,
        };
        reference.validate()?;
        Ok(reference)
    }

    pub fn validate(&self) -> Result<(), DomainError> {
        validate_unit("evidence strength", self.strength)?;
        validate_unit("evidence freshness", self.freshness)?;
        validate_unit("evidence reliability", self.reliability)?;
        validate_unit("evidence novelty", self.novelty)?;
        validate_unit("evidence relevance", self.relevance)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CounterSignal {
    pub evidence: EvidenceReference,
    pub summary: String,
}

impl CounterSignal {
    pub fn new(
        evidence: EvidenceReference,
        summary: impl Into<String>,
    ) -> Result<Self, DomainError> {
        evidence.validate()?;
        if evidence.direction != EvidenceDirection::Counter {
            return Err(DomainError::NotCounterSignal);
        }

        let summary = summary.into();
        if summary.trim().is_empty() {
            return Err(DomainError::EmptyCounterSignalSummary);
        }

        Ok(Self { evidence, summary })
    }
}

pub(crate) fn validate_unit(field: &'static str, value: Decimal) -> Result<(), DomainError> {
    if value < Decimal::ZERO || value > Decimal::ONE {
        return Err(DomainError::OutOfRange { field, value });
    }
    Ok(())
}
