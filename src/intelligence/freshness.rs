use rust_decimal::Decimal;
use time::OffsetDateTime;

use super::{IntelligenceError, Score};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FreshnessPolicy {
    window_seconds: i64,
}

impl FreshnessPolicy {
    pub fn linear(window_seconds: i64) -> Result<Self, IntelligenceError> {
        if window_seconds <= 0 {
            return Err(IntelligenceError::InvalidFreshnessWindow);
        }
        Ok(Self { window_seconds })
    }

    pub fn calculate(
        &self,
        event_time: OffsetDateTime,
        _observed_at: OffsetDateTime,
        evaluated_at: OffsetDateTime,
    ) -> Result<Score, IntelligenceError> {
        if event_time > evaluated_at {
            return Err(IntelligenceError::FutureObservation);
        }

        let age_seconds = evaluated_at
            .unix_timestamp()
            .saturating_sub(event_time.unix_timestamp());
        let age = Decimal::from(age_seconds);
        let window = Decimal::from(self.window_seconds);
        let remaining = (Decimal::ONE - age / window).max(Decimal::ZERO);
        Score::new(remaining)
    }
}
