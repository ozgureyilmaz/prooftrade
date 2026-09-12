mod error;
mod events;
mod evidence;
mod freshness;
mod health;
mod lineage;
mod query;
mod service;
pub mod sources;

pub use error::IntelligenceError;
pub use events::{EventFingerprint, EventIdentity};
pub use evidence::{
    AssetImpact, EvidencePayload, IntelligenceDirection, IntelligenceEvidence, IntelligenceSource,
    MarxEvidence, NewsEvidence, PredictionMarketEvidence, Probability, RedditEvidence, Score,
    SignedValue, SourceReference,
};
pub use freshness::FreshnessPolicy;
pub use health::{SourceErrorCategory, SourceHealthRegistry, SourceHealthStatus, SourceHealthView};
pub use lineage::{IngestDisposition, IngestOutcome, LineageLedger};
pub use query::BoundedEvidenceQuery;
pub use service::{IntelligenceService, IntelligenceSnapshot};
