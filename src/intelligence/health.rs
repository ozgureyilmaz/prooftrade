use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use super::IntelligenceSource;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum SourceHealthStatus {
    Unknown,
    Healthy,
    Degraded,
    Unavailable,
    Disabled,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum SourceErrorCategory {
    Unavailable,
    Timeout,
    RateLimited,
    MalformedResponse,
    Unauthorized,
    CapabilityMissing,
    InvalidData,
    ProcessFailure,
    Disabled,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct SourceHealth {
    status: SourceHealthStatus,
    last_success: Option<OffsetDateTime>,
    last_attempt: Option<OffsetDateTime>,
    last_error: Option<SourceErrorCategory>,
    consecutive_failures: u32,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SourceHealthView {
    pub source: IntelligenceSource,
    pub status: SourceHealthStatus,
    pub last_success: Option<OffsetDateTime>,
    pub last_attempt: Option<OffsetDateTime>,
    pub last_error: Option<SourceErrorCategory>,
    pub consecutive_failures: u32,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct SourceHealthRegistry {
    sources: BTreeMap<IntelligenceSource, SourceHealth>,
}

impl SourceHealthRegistry {
    pub fn new() -> Self {
        let mut registry = Self::default();
        for source in [
            IntelligenceSource::AtkNews,
            IntelligenceSource::Polymarket,
            IntelligenceSource::Reddit,
            IntelligenceSource::MarxFinance,
        ] {
            registry.sources.insert(
                source,
                SourceHealth {
                    status: SourceHealthStatus::Unknown,
                    last_success: None,
                    last_attempt: None,
                    last_error: None,
                    consecutive_failures: 0,
                },
            );
        }
        registry
    }

    pub fn record_success(&mut self, source: IntelligenceSource, at: OffsetDateTime) {
        let health = self.entry(source);
        health.status = SourceHealthStatus::Healthy;
        health.last_success = Some(at);
        health.last_attempt = Some(at);
        health.last_error = None;
        health.consecutive_failures = 0;
    }

    pub fn record_failure(
        &mut self,
        source: IntelligenceSource,
        at: OffsetDateTime,
        category: SourceErrorCategory,
    ) {
        let health = self.entry(source);
        health.status = match category {
            SourceErrorCategory::Unavailable
            | SourceErrorCategory::Unauthorized
            | SourceErrorCategory::CapabilityMissing
            | SourceErrorCategory::ProcessFailure => SourceHealthStatus::Unavailable,
            SourceErrorCategory::Disabled => SourceHealthStatus::Disabled,
            _ => SourceHealthStatus::Degraded,
        };
        health.last_attempt = Some(at);
        health.last_error = Some(category);
        health.consecutive_failures = health.consecutive_failures.saturating_add(1);
    }

    pub fn record_disabled(&mut self, source: IntelligenceSource, at: OffsetDateTime) {
        let health = self.entry(source);
        health.status = SourceHealthStatus::Disabled;
        health.last_attempt = Some(at);
        health.last_error = None;
        health.consecutive_failures = 0;
    }

    pub fn status(&self, source: IntelligenceSource) -> SourceHealthStatus {
        self.sources
            .get(&source)
            .map(|health| health.status)
            .unwrap_or(SourceHealthStatus::Unknown)
    }

    pub fn last_error(&self, source: IntelligenceSource) -> Option<SourceErrorCategory> {
        self.sources
            .get(&source)
            .and_then(|health| health.last_error)
    }

    pub fn views(&self) -> Vec<SourceHealthView> {
        self.sources
            .iter()
            .map(|(source, health)| SourceHealthView {
                source: *source,
                status: health.status,
                last_success: health.last_success,
                last_attempt: health.last_attempt,
                last_error: health.last_error,
                consecutive_failures: health.consecutive_failures,
            })
            .collect()
    }

    fn entry(&mut self, source: IntelligenceSource) -> &mut SourceHealth {
        self.sources.entry(source).or_insert(SourceHealth {
            status: SourceHealthStatus::Unknown,
            last_success: None,
            last_attempt: None,
            last_error: None,
            consecutive_failures: 0,
        })
    }
}
