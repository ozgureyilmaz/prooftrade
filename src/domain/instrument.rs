use serde::{Deserialize, Serialize};

use super::DomainError;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MarketType {
    Spot,
}

#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct Instrument(String);

impl Instrument {
    pub fn spot(base: &str, quote: &str) -> Result<Self, DomainError> {
        Self::parse(&format!("{base}-{quote}"))
    }

    pub fn parse(symbol: &str) -> Result<Self, DomainError> {
        let mut parts = symbol.split('-');
        let Some(base) = parts.next() else {
            return Err(DomainError::InvalidInstrument);
        };
        let Some(quote) = parts.next() else {
            return Err(DomainError::InvalidInstrument);
        };

        if parts.next().is_some()
            || base.is_empty()
            || quote.is_empty()
            || !base
                .chars()
                .all(|character| character.is_ascii_uppercase() || character.is_ascii_digit())
            || !quote
                .chars()
                .all(|character| character.is_ascii_uppercase() || character.is_ascii_digit())
        {
            return Err(DomainError::InvalidInstrument);
        }

        Ok(Self(symbol.to_owned()))
    }

    pub fn symbol(&self) -> &str {
        &self.0
    }

    pub const fn market_type(&self) -> MarketType {
        MarketType::Spot
    }

    pub fn validate(&self) -> Result<(), DomainError> {
        Self::parse(&self.0).map(|_| ())
    }
}

impl TryFrom<&str> for Instrument {
    type Error = DomainError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}

impl std::fmt::Display for Instrument {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}
