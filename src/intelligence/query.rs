use std::collections::BTreeSet;

use time::OffsetDateTime;

use crate::domain::Instrument;

use super::{IntelligenceError, IntelligenceEvidence};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BoundedEvidenceQuery {
    as_of: OffsetDateTime,
    instruments: BTreeSet<Instrument>,
    max_age_seconds: Option<i64>,
    max_results: usize,
    max_per_source: usize,
}

impl BoundedEvidenceQuery {
    pub fn new(
        as_of: OffsetDateTime,
        max_results: usize,
        max_per_source: usize,
    ) -> Result<Self, IntelligenceError> {
        if max_results == 0 || max_per_source == 0 {
            return Err(IntelligenceError::EmptyField {
                field: "query limits",
            });
        }
        Ok(Self {
            as_of,
            instruments: BTreeSet::new(),
            max_age_seconds: None,
            max_results,
            max_per_source,
        })
    }

    pub fn with_instruments(mut self, instruments: impl IntoIterator<Item = Instrument>) -> Self {
        self.instruments.extend(instruments);
        self
    }

    pub fn with_max_age_seconds(mut self, max_age_seconds: i64) -> Result<Self, IntelligenceError> {
        if max_age_seconds < 0 {
            return Err(IntelligenceError::NegativeValue {
                field: "max_age_seconds",
            });
        }
        self.max_age_seconds = Some(max_age_seconds);
        Ok(self)
    }

    pub(crate) fn max_results(&self) -> usize {
        self.max_results
    }

    pub(crate) fn max_per_source(&self) -> usize {
        self.max_per_source
    }

    pub(crate) fn max_age_seconds(&self) -> Option<i64> {
        self.max_age_seconds
    }

    pub(crate) fn instruments(&self) -> &BTreeSet<Instrument> {
        &self.instruments
    }

    pub(crate) fn as_of(&self) -> OffsetDateTime {
        self.as_of
    }

    pub(crate) fn matches(&self, evidence: &IntelligenceEvidence) -> bool {
        if evidence
            .event_time
            .is_some_and(|event_time| event_time > self.as_of)
        {
            return false;
        }
        if let (Some(max_age), Some(event_time)) = (self.max_age_seconds, evidence.event_time) {
            let age = self
                .as_of
                .unix_timestamp()
                .saturating_sub(event_time.unix_timestamp());
            if age > max_age {
                return false;
            }
        }
        self.instruments.is_empty()
            || evidence
                .affected_assets
                .iter()
                .any(|impact| self.instruments.contains(&impact.instrument))
    }

    pub(crate) fn targets_instrument(&self, instrument: &Instrument) -> bool {
        !self.instruments.is_empty() && self.instruments.contains(instrument)
    }
}
