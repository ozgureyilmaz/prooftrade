use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use super::{DomainError, Instrument, PositionId, TradeAction, TradeDecision};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum PositionState {
    Flat {
        instrument: Instrument,
        updated_at: OffsetDateTime,
    },
    Open {
        position_id: PositionId,
        instrument: Instrument,
        quantity: Decimal,
        average_entry_price: Decimal,
        opened_at: OffsetDateTime,
        updated_at: OffsetDateTime,
    },
}

impl PositionState {
    pub fn flat(instrument: Instrument, updated_at: OffsetDateTime) -> Self {
        Self::Flat {
            instrument,
            updated_at,
        }
    }

    pub fn open(
        position_id: PositionId,
        instrument: Instrument,
        quantity: Decimal,
        average_entry_price: Decimal,
        opened_at: OffsetDateTime,
        updated_at: OffsetDateTime,
    ) -> Result<Self, DomainError> {
        let position = Self::Open {
            position_id,
            instrument,
            quantity,
            average_entry_price,
            opened_at,
            updated_at,
        };
        position.validate()?;
        Ok(position)
    }

    pub fn instrument(&self) -> &Instrument {
        match self {
            Self::Flat { instrument, .. } | Self::Open { instrument, .. } => instrument,
        }
    }

    pub fn is_open(&self) -> bool {
        matches!(self, Self::Open { .. })
    }

    pub fn validate(&self) -> Result<(), DomainError> {
        self.instrument().validate()?;
        if let Self::Open {
            quantity,
            average_entry_price,
            opened_at,
            updated_at,
            ..
        } = self
        {
            if *quantity <= Decimal::ZERO {
                return Err(DomainError::InvalidQuantity);
            }
            if *average_entry_price <= Decimal::ZERO {
                return Err(DomainError::InvalidPrice);
            }
            if opened_at > updated_at {
                return Err(DomainError::InvalidTimestampOrder);
            }
        }
        Ok(())
    }

    pub fn validate_decision(&self, decision: &TradeDecision) -> Result<(), DomainError> {
        self.validate()?;
        decision.validate()?;
        if decision.instrument != *self.instrument() {
            return Err(DomainError::InstrumentMismatch);
        }
        if decision.action == TradeAction::Sell && !self.is_open() {
            return Err(DomainError::SellWithoutInventory);
        }
        Ok(())
    }
}
