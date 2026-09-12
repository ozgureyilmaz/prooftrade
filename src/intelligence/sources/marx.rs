use super::{IntelligenceSourceAdapter, SourceError};
use crate::intelligence::{BoundedEvidenceQuery, IntelligenceEvidence, IntelligenceSource};

pub trait MarxReader {
    fn collect_marx(
        &mut self,
        query: &BoundedEvidenceQuery,
    ) -> Result<Vec<IntelligenceEvidence>, SourceError>;
}

pub struct MarxAdapter<R> {
    reader: R,
}

impl<R> MarxAdapter<R> {
    pub fn new(reader: R) -> Self {
        Self { reader }
    }
}

impl<R: MarxReader> IntelligenceSourceAdapter for MarxAdapter<R> {
    fn source(&self) -> IntelligenceSource {
        IntelligenceSource::MarxFinance
    }

    fn collect(
        &mut self,
        query: &BoundedEvidenceQuery,
    ) -> Result<Vec<IntelligenceEvidence>, SourceError> {
        self.reader.collect_marx(query)
    }
}
