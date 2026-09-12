use rust_decimal::Decimal;
use thiserror::Error;

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum IntelligenceError {
    #[error("{field} must be between zero and one: {value}")]
    ScoreOutOfRange { field: &'static str, value: Decimal },
    #[error("{field} cannot be empty")]
    EmptyField { field: &'static str },
    #[error("event time cannot be later than observation time")]
    FutureEventTime,
    #[error("{field} must be non-negative")]
    NegativeValue { field: &'static str },
    #[error("evidence source does not match its typed payload")]
    PayloadSourceMismatch,
    #[error("invalid event fingerprint")]
    InvalidEventFingerprint,
    #[error("event time bucket must be greater than zero")]
    InvalidTimeBucket,
    #[error("freshness window must be greater than zero")]
    InvalidFreshnessWindow,
    #[error("event time is in the future relative to evaluation time")]
    FutureObservation,
    #[error("probability history capacity must be greater than zero")]
    InvalidHistoryCapacity,
    #[error("probability observation timestamps must be strictly increasing")]
    OutOfOrderObservation,
}
