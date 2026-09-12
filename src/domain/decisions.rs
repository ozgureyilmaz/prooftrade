use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use super::{
    DecisionId, DomainError, EvidenceReference, Instrument, PositionState, evidence::validate_unit,
};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum TradeAction {
    Buy,
    Sell,
    Hold,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Urgency {
    Low,
    Normal,
    High,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Invalidation {
    pub description: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TradeDecision {
    pub decision_id: DecisionId,
    pub instrument: Instrument,
    pub action: TradeAction,
    pub confidence: Decimal,
    pub support_strength: Decimal,
    pub counter_signal_strength: Decimal,
    pub expected_horizon_secs: Option<u64>,
    pub desired_exposure_pct: Option<Decimal>,
    pub requested_notional: Option<Decimal>,
    pub desired_reduction_pct: Option<Decimal>,
    pub urgency: Urgency,
    pub max_acceptable_price: Option<Decimal>,
    pub why_trade: Vec<String>,
    pub why_not_trade: Vec<String>,
    pub reconsider_if: Vec<String>,
    pub invalidation: Option<Invalidation>,
    pub evidence_refs: Vec<EvidenceReference>,
    pub created_at: OffsetDateTime,
}

impl TradeDecision {
    pub fn buy(
        decision_id: DecisionId,
        instrument: Instrument,
        created_at: OffsetDateTime,
        requested_notional: Decimal,
        desired_exposure_pct: Decimal,
    ) -> Self {
        Self {
            action: TradeAction::Buy,
            requested_notional: Some(requested_notional),
            desired_exposure_pct: Some(desired_exposure_pct),
            ..Self::base(decision_id, instrument, created_at, TradeAction::Buy)
        }
    }

    pub fn sell(
        decision_id: DecisionId,
        instrument: Instrument,
        created_at: OffsetDateTime,
        desired_reduction_pct: Decimal,
    ) -> Self {
        Self {
            action: TradeAction::Sell,
            desired_reduction_pct: Some(desired_reduction_pct),
            ..Self::base(decision_id, instrument, created_at, TradeAction::Sell)
        }
    }

    pub fn hold(
        decision_id: DecisionId,
        instrument: Instrument,
        created_at: OffsetDateTime,
        reason: impl Into<String>,
    ) -> Self {
        Self {
            action: TradeAction::Hold,
            why_not_trade: vec![reason.into()],
            ..Self::base(decision_id, instrument, created_at, TradeAction::Hold)
        }
    }

    pub fn validate(&self) -> Result<(), DomainError> {
        validate_unit("confidence", self.confidence)?;
        validate_unit("support strength", self.support_strength)?;
        validate_unit("counter-signal strength", self.counter_signal_strength)?;

        if self.expected_horizon_secs == Some(0) {
            return Err(DomainError::InvalidHorizon);
        }

        if let Some(price) = self.max_acceptable_price {
            if price <= Decimal::ZERO {
                return Err(DomainError::InvalidPrice);
            }
        }

        match self.action {
            TradeAction::Buy => {
                require_positive(self.requested_notional, "requested notional")?;
                require_unit(self.desired_exposure_pct, "desired exposure percentage")?;
                if self.desired_reduction_pct.is_some() {
                    return Err(DomainError::InconsistentDecisionExposure);
                }
            }
            TradeAction::Sell => {
                require_positive(self.desired_reduction_pct, "desired reduction percentage")?;
                validate_unit_option(self.desired_reduction_pct, "desired reduction percentage")?;
                if self.requested_notional.is_some() || self.desired_exposure_pct.is_some() {
                    return Err(DomainError::InconsistentDecisionExposure);
                }
            }
            TradeAction::Hold => {
                if !self
                    .why_not_trade
                    .iter()
                    .any(|reason| !reason.trim().is_empty())
                {
                    return Err(DomainError::MissingHoldReason);
                }
                if self.requested_notional.is_some()
                    || self.desired_exposure_pct.is_some()
                    || self.desired_reduction_pct.is_some()
                    || self.max_acceptable_price.is_some()
                {
                    return Err(DomainError::InconsistentDecisionExposure);
                }
            }
        }

        if let Some(invalidation) = &self.invalidation {
            if invalidation.description.trim().is_empty() {
                return Err(DomainError::EmptyIdentifier("invalidation.description"));
            }
        }

        for evidence in &self.evidence_refs {
            evidence.validate()?;
            if let Some(instrument) = &evidence.instrument {
                if instrument != &self.instrument {
                    return Err(DomainError::InstrumentMismatch);
                }
            }
        }

        Ok(())
    }

    pub fn validate_for_position(&self, position: &PositionState) -> Result<(), DomainError> {
        self.validate().and_then(|_| {
            if self.instrument != *position.instrument() {
                return Err(DomainError::InstrumentMismatch);
            }
            if self.action == TradeAction::Sell && !position.is_open() {
                return Err(DomainError::SellWithoutInventory);
            }
            Ok(())
        })
    }

    fn base(
        decision_id: DecisionId,
        instrument: Instrument,
        created_at: OffsetDateTime,
        action: TradeAction,
    ) -> Self {
        Self {
            decision_id,
            instrument,
            action,
            confidence: Decimal::ZERO,
            support_strength: Decimal::ZERO,
            counter_signal_strength: Decimal::ZERO,
            expected_horizon_secs: None,
            desired_exposure_pct: None,
            requested_notional: None,
            desired_reduction_pct: None,
            urgency: Urgency::Normal,
            max_acceptable_price: None,
            why_trade: Vec::new(),
            why_not_trade: Vec::new(),
            reconsider_if: Vec::new(),
            invalidation: None,
            evidence_refs: Vec::new(),
            created_at,
        }
    }
}

fn require_positive(value: Option<Decimal>, field: &'static str) -> Result<(), DomainError> {
    match value {
        Some(value) if value > Decimal::ZERO => Ok(()),
        _ => Err(DomainError::NotPositive(field)),
    }
}

fn require_unit(value: Option<Decimal>, field: &'static str) -> Result<(), DomainError> {
    let Some(value) = value else {
        return Err(DomainError::NotPositive(field));
    };
    if value <= Decimal::ZERO {
        return Err(DomainError::NotPositive(field));
    }
    validate_unit(field, value)
}

fn validate_unit_option(value: Option<Decimal>, field: &'static str) -> Result<(), DomainError> {
    if let Some(value) = value {
        validate_unit(field, value)?;
    }
    Ok(())
}
