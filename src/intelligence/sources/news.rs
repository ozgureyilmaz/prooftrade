use super::{IntelligenceSourceAdapter, SourceError};
use crate::intelligence::SourceErrorCategory;
use crate::intelligence::{BoundedEvidenceQuery, IntelligenceEvidence, IntelligenceSource};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NewsCapability {
    Available,
    Missing,
}

impl NewsCapability {
    pub const fn from_atk_news_available(available: bool) -> Self {
        if available {
            Self::Available
        } else {
            Self::Missing
        }
    }

    pub const fn from_atk_capabilities(report: &crate::atk::CapabilityReport) -> Self {
        Self::from_atk_news_available(report.news)
    }
}

pub trait NewsReader {
    fn collect_news(
        &mut self,
        query: &BoundedEvidenceQuery,
    ) -> Result<Vec<IntelligenceEvidence>, SourceError>;
}

impl NewsReader for () {
    fn collect_news(
        &mut self,
        _query: &BoundedEvidenceQuery,
    ) -> Result<Vec<IntelligenceEvidence>, SourceError> {
        Err(SourceError::new(
            IntelligenceSource::AtkNews,
            SourceErrorCategory::Unavailable,
        ))
    }
}

pub struct NewsAdapter<R> {
    capability: NewsCapability,
    reader: Option<R>,
}

impl<R> NewsAdapter<R> {
    pub fn without_reader(capability: NewsCapability) -> Self {
        Self {
            capability,
            reader: None,
        }
    }

    pub fn with_reader(capability: NewsCapability, reader: R) -> Self {
        Self {
            capability,
            reader: Some(reader),
        }
    }
}

impl<R: NewsReader> IntelligenceSourceAdapter for NewsAdapter<R> {
    fn source(&self) -> IntelligenceSource {
        IntelligenceSource::AtkNews
    }

    fn collect(
        &mut self,
        query: &BoundedEvidenceQuery,
    ) -> Result<Vec<IntelligenceEvidence>, SourceError> {
        if self.capability == NewsCapability::Missing {
            return Err(SourceError::new(
                IntelligenceSource::AtkNews,
                SourceErrorCategory::CapabilityMissing,
            ));
        }
        self.reader.as_mut().map_or_else(
            || {
                Err(SourceError::new(
                    IntelligenceSource::AtkNews,
                    SourceErrorCategory::Unavailable,
                ))
            },
            |reader| reader.collect_news(query),
        )
    }
}
