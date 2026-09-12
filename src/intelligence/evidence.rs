use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::domain::{EvidenceId, Instrument, LineageId, SourceEventId};

use super::{EventFingerprint, IntelligenceError};

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IntelligenceSource {
    AtkNews,
    Polymarket,
    Reddit,
    MarxFinance,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IntelligenceDirection {
    Bullish,
    Bearish,
    Neutral,
    Mixed,
    Unclear,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct Score(Decimal);

impl Score {
    pub fn new(value: Decimal) -> Result<Self, IntelligenceError> {
        let score = Self(value);
        score.validate("score")?;
        Ok(score)
    }

    pub fn value(self) -> Decimal {
        self.0
    }

    pub fn validate(self, field: &'static str) -> Result<(), IntelligenceError> {
        if self.0 < Decimal::ZERO || self.0 > Decimal::ONE {
            return Err(IntelligenceError::ScoreOutOfRange {
                field,
                value: self.0,
            });
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct Probability(Decimal);

impl Probability {
    pub fn new(value: Decimal) -> Result<Self, IntelligenceError> {
        let probability = Self(value);
        if value < Decimal::ZERO || value > Decimal::ONE {
            return Err(IntelligenceError::ScoreOutOfRange {
                field: "probability",
                value,
            });
        }
        Ok(probability)
    }

    pub fn value(self) -> Decimal {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct SignedValue(Decimal);

impl SignedValue {
    pub fn new(value: Decimal) -> Result<Self, IntelligenceError> {
        if value < -Decimal::ONE || value > Decimal::ONE {
            return Err(IntelligenceError::ScoreOutOfRange {
                field: "signed value",
                value,
            });
        }
        Ok(Self(value))
    }

    pub fn value(self) -> Decimal {
        self.0
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct SourceReference(String);

impl SourceReference {
    pub fn new(value: impl Into<String>) -> Result<Self, IntelligenceError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(IntelligenceError::EmptyField {
                field: "source_reference",
            });
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AssetImpact {
    pub instrument: Instrument,
    pub relevance: Score,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct NewsEvidence {
    pub headline: String,
    pub summary: Option<String>,
    pub category: Option<String>,
    pub importance: Option<Score>,
    pub published_at: Option<OffsetDateTime>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PredictionMarketEvidence {
    pub market_id: String,
    pub token_id: String,
    pub question: String,
    pub implied_probability: Probability,
    pub probability_delta: Option<SignedValue>,
    pub liquidity: Option<Decimal>,
    pub volume: Option<Decimal>,
    pub spread: Option<Decimal>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RedditEvidence {
    pub post_id: String,
    pub communities: Vec<String>,
    pub event_type: Option<String>,
    pub supporting_signals: Vec<String>,
    pub counter_signals: Vec<String>,
    pub context_comments: Option<u32>,
    pub event_relevance: Score,
    pub mention_velocity: Option<Decimal>,
    pub unique_author_velocity: Option<Decimal>,
    pub engagement_velocity: Option<Decimal>,
    pub sentiment: Option<SignedValue>,
    pub sentiment_delta: Option<SignedValue>,
    pub sentiment_dispersion: Option<Score>,
    pub cross_community_confirmation: Option<Score>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct MarxEvidence {
    pub agent_id: String,
    pub thesis: String,
    pub stance: IntelligenceDirection,
    pub supporting_arguments: Vec<String>,
    pub counter_arguments: Vec<String>,
    pub confidence: Option<Score>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum EvidencePayload {
    News(NewsEvidence),
    PredictionMarket(PredictionMarketEvidence),
    Reddit(RedditEvidence),
    Marx(MarxEvidence),
}

impl EvidencePayload {
    fn source(&self) -> IntelligenceSource {
        match self {
            Self::News(_) => IntelligenceSource::AtkNews,
            Self::PredictionMarket(_) => IntelligenceSource::Polymarket,
            Self::Reddit(_) => IntelligenceSource::Reddit,
            Self::Marx(_) => IntelligenceSource::MarxFinance,
        }
    }

    fn validate(&self) -> Result<(), IntelligenceError> {
        match self {
            Self::News(news) if news.headline.trim().is_empty() => {
                Err(IntelligenceError::EmptyField { field: "headline" })
            }
            Self::News(news) => {
                if let Some(importance) = news.importance {
                    importance.validate("news importance")?;
                }
                Ok(())
            }
            Self::PredictionMarket(market) => {
                validate_non_empty(&market.market_id, "market_id")?;
                validate_non_empty(&market.token_id, "token_id")?;
                validate_non_empty(&market.question, "question")?;
                validate_non_negative(market.liquidity, "liquidity")?;
                validate_non_negative(market.volume, "volume")?;
                validate_non_negative(market.spread, "spread")
            }
            Self::Reddit(reddit) => {
                validate_non_empty(&reddit.post_id, "post_id")?;
                reddit.event_relevance.validate("event relevance")?;
                for community in &reddit.communities {
                    validate_non_empty(community, "community")?;
                }
                validate_non_negative(reddit.mention_velocity, "mention velocity")?;
                validate_non_negative(reddit.unique_author_velocity, "unique author velocity")?;
                validate_non_negative(reddit.engagement_velocity, "engagement velocity")?;
                if let Some(dispersion) = reddit.sentiment_dispersion {
                    dispersion.validate("sentiment dispersion")?;
                }
                if let Some(confirmation) = reddit.cross_community_confirmation {
                    confirmation.validate("cross-community confirmation")?;
                }
                Ok(())
            }
            Self::Marx(marx) => {
                validate_non_empty(&marx.agent_id, "agent_id")?;
                validate_non_empty(&marx.thesis, "thesis")?;
                if let Some(confidence) = marx.confidence {
                    confidence.validate("Marx confidence")?;
                }
                Ok(())
            }
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct IntelligenceEvidence {
    pub evidence_id: EvidenceId,
    pub source: IntelligenceSource,
    pub source_event_id: SourceEventId,
    pub event_time: Option<OffsetDateTime>,
    pub observed_at: OffsetDateTime,
    pub affected_assets: Vec<AssetImpact>,
    pub direction: IntelligenceDirection,
    pub strength: Option<Score>,
    pub freshness: Option<Score>,
    pub reliability: Option<Score>,
    pub novelty: Option<Score>,
    pub summary: String,
    pub source_reference: Option<SourceReference>,
    pub event_fingerprint: EventFingerprint,
    pub lineage_id: Option<LineageId>,
    pub payload: EvidencePayload,
}

fn validate_non_empty(value: &str, field: &'static str) -> Result<(), IntelligenceError> {
    if value.trim().is_empty() {
        return Err(IntelligenceError::EmptyField { field });
    }
    Ok(())
}

fn validate_non_negative(
    value: Option<Decimal>,
    field: &'static str,
) -> Result<(), IntelligenceError> {
    if value.is_some_and(|value| value < Decimal::ZERO) {
        return Err(IntelligenceError::NegativeValue { field });
    }
    Ok(())
}

impl IntelligenceEvidence {
    pub fn validate(&self) -> Result<(), IntelligenceError> {
        if self.source != self.payload.source() {
            return Err(IntelligenceError::PayloadSourceMismatch);
        }
        if self.summary.trim().is_empty() {
            return Err(IntelligenceError::EmptyField { field: "summary" });
        }
        if self.affected_assets.is_empty() {
            return Err(IntelligenceError::EmptyField {
                field: "affected_assets",
            });
        }
        if self
            .event_time
            .is_some_and(|event_time| event_time > self.observed_at)
        {
            return Err(IntelligenceError::FutureEventTime);
        }
        for (field, score) in [
            ("strength", self.strength),
            ("freshness", self.freshness),
            ("reliability", self.reliability),
            ("novelty", self.novelty),
        ] {
            if let Some(score) = score {
                score.validate(field)?;
            }
        }
        for impact in &self.affected_assets {
            impact
                .instrument
                .validate()
                .map_err(|_| IntelligenceError::EmptyField {
                    field: "affected_assets.instrument",
                })?;
            impact.relevance.validate("asset relevance")?;
        }
        self.payload.validate()
    }
}
