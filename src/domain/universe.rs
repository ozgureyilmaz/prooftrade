use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use super::{DomainError, Instrument};

const INITIAL_SYMBOLS: [&str; 4] = ["BTC-USDT", "ETH-USDT", "SOL-USDT", "HYPE-USDT"];

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TradingUniverse {
    instruments: Vec<Instrument>,
}

impl TradingUniverse {
    pub fn initial() -> Self {
        let instruments = INITIAL_SYMBOLS
            .into_iter()
            .map(|symbol| Instrument::parse(symbol).expect("initial symbol must be valid"))
            .collect();

        Self { instruments }
    }

    pub fn new(instruments: Vec<Instrument>) -> Result<Self, DomainError> {
        let universe = Self { instruments };
        universe.validate()?;
        Ok(universe)
    }

    pub fn validate(&self) -> Result<(), DomainError> {
        if self.instruments.is_empty() {
            return Err(DomainError::EmptyUniverse);
        }

        let mut symbols = HashSet::with_capacity(self.instruments.len());
        for instrument in &self.instruments {
            if !symbols.insert(instrument.symbol()) {
                return Err(DomainError::DuplicateInstrument(instrument.to_string()));
            }
        }

        Ok(())
    }

    pub fn instruments(&self) -> &[Instrument] {
        &self.instruments
    }

    pub fn contains(&self, instrument: &Instrument) -> bool {
        self.instruments.contains(instrument)
    }
}

impl Default for TradingUniverse {
    fn default() -> Self {
        Self::initial()
    }
}
