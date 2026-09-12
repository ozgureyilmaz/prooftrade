use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use time::OffsetDateTime;

use crate::domain::Instrument;

use super::IntelligenceError;

#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct EventFingerprint(String);

impl EventFingerprint {
    pub fn from_canonical_key(value: impl Into<String>) -> Result<Self, IntelligenceError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(IntelligenceError::InvalidEventFingerprint);
        }

        let mut hasher = Sha256::new();
        hasher.update(value.as_bytes());
        let digest = hasher.finalize();
        Ok(Self(format!("sha256:{digest:x}")))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EventIdentity {
    event_kind: String,
    canonical_topic: String,
    named_entities: Vec<String>,
    affected_assets: Vec<Instrument>,
    event_time: Option<OffsetDateTime>,
    time_bucket_seconds: i64,
}

impl EventIdentity {
    pub fn new(
        event_kind: impl Into<String>,
        canonical_topic: impl Into<String>,
        named_entities: Vec<String>,
        affected_assets: Vec<Instrument>,
        event_time: Option<OffsetDateTime>,
        time_bucket_seconds: i64,
    ) -> Result<Self, IntelligenceError> {
        let event_kind = normalize_required(event_kind.into(), "event_kind")?;
        let canonical_topic = normalize_required(canonical_topic.into(), "canonical_topic")?;
        if affected_assets.is_empty() {
            return Err(IntelligenceError::EmptyField {
                field: "affected_assets",
            });
        }
        if time_bucket_seconds <= 0 {
            return Err(IntelligenceError::InvalidTimeBucket);
        }

        let named_entities = normalized_strings(named_entities);
        let mut assets = affected_assets;
        assets.sort();
        assets.dedup();

        Ok(Self {
            event_kind,
            canonical_topic,
            named_entities,
            affected_assets: assets,
            event_time,
            time_bucket_seconds,
        })
    }

    pub fn fingerprint(&self) -> Result<EventFingerprint, IntelligenceError> {
        let event_bucket = self
            .event_time
            .map(|time| time.unix_timestamp().div_euclid(self.time_bucket_seconds));
        let assets = self
            .affected_assets
            .iter()
            .map(Instrument::symbol)
            .collect::<Vec<_>>()
            .join(",");
        let entities = self.named_entities.join(",");
        let canonical = format!(
            "v1|kind={}|topic={}|entities={}|assets={}|bucket={:?}",
            self.event_kind, self.canonical_topic, entities, assets, event_bucket
        );
        EventFingerprint::from_canonical_key(canonical)
    }
}

fn normalize_required(value: String, field: &'static str) -> Result<String, IntelligenceError> {
    let value = value.trim().to_ascii_lowercase();
    if value.is_empty() {
        return Err(IntelligenceError::EmptyField { field });
    }
    Ok(value)
}

fn normalized_strings(values: Vec<String>) -> Vec<String> {
    BTreeSet::from_iter(
        values
            .into_iter()
            .map(|value| value.trim().to_ascii_lowercase())
            .filter(|value| !value.is_empty()),
    )
    .into_iter()
    .collect()
}
