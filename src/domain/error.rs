use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum DomainError {
    #[error("instrument symbol must contain one base and one quote asset")]
    InvalidInstrument,
    #[error("trading universe cannot be empty")]
    EmptyUniverse,
    #[error("trading universe contains a duplicate instrument: {0}")]
    DuplicateInstrument(String),
    #[error("identifier cannot be empty: {0}")]
    EmptyIdentifier(&'static str),
    #[error("value for {field} must be between zero and one: {value}")]
    OutOfRange {
        field: &'static str,
        value: rust_decimal::Decimal,
    },
    #[error("value must be greater than zero: {0}")]
    NotPositive(&'static str),
    #[error("position timestamps are out of order")]
    InvalidTimestampOrder,
    #[error("position quantity cannot be negative")]
    InvalidQuantity,
    #[error("price must be greater than zero")]
    InvalidPrice,
    #[error("trade horizon must be greater than zero")]
    InvalidHorizon,
    #[error("opportunity rank must be greater than zero")]
    InvalidOpportunityRank,
    #[error("opportunities contain a duplicate instrument: {0}")]
    DuplicateOpportunityInstrument(String),
    #[error("opportunities contain a duplicate rank: {0}")]
    DuplicateOpportunityRank(u32),
    #[error("counter-signal evidence must have counter direction")]
    NotCounterSignal,
    #[error("counter-signal summary cannot be empty")]
    EmptyCounterSignalSummary,
    #[error("decision action is inconsistent with its exposure fields")]
    InconsistentDecisionExposure,
    #[error("HOLD decisions must explain why no trade is being taken")]
    MissingHoldReason,
    #[error("SELL is invalid without an open position")]
    SellWithoutInventory,
    #[error("decision instrument does not match the position instrument")]
    InstrumentMismatch,
}
