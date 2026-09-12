use thiserror::Error;

use super::SourceErrorCategory;
use super::{BoundedEvidenceQuery, IntelligenceEvidence, IntelligenceSource};

pub mod marx;
pub mod news;
pub mod polymarket;
pub mod reddit;

#[derive(Clone, Debug, Error, Eq, PartialEq)]
#[error("{provider:?} source failed: {category:?}")]
pub struct SourceError {
    provider: IntelligenceSource,
    category: SourceErrorCategory,
}

impl SourceError {
    pub fn new(source: IntelligenceSource, category: SourceErrorCategory) -> Self {
        Self {
            provider: source,
            category,
        }
    }

    pub fn source(&self) -> IntelligenceSource {
        self.provider
    }

    pub fn category(&self) -> SourceErrorCategory {
        self.category
    }
}

pub trait IntelligenceSourceAdapter {
    fn source(&self) -> IntelligenceSource;

    /// Whether this source is allowed to participate in collection.
    /// Existing adapters remain enabled unless they opt into the gate.
    fn is_enabled(&self) -> bool {
        true
    }

    fn set_enabled(&mut self, _enabled: bool) {}

    fn collect(
        &mut self,
        query: &BoundedEvidenceQuery,
    ) -> Result<Vec<IntelligenceEvidence>, SourceError>;
}
