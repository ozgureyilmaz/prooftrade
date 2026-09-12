//! Deterministic capital authority between proposed agent intent and execution.

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, RwLock};
use thiserror::Error;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::decision::MarketContext;
use crate::domain::{DecisionId, Instrument, PositionState, TradeAction, TradeDecision};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum OperatingMode {
    Simulation,
    Demo,
    Live,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum SafetyState {
    Normal,
    Safe,
    Killed,
}

#[derive(Clone, Debug)]
pub struct SafetyStateStore {
    state: Arc<RwLock<SafetyState>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Error)]
pub enum SafetyStateStoreError {
    #[error("safety state lock is poisoned")]
    LockPoisoned,
}

impl SafetyStateStore {
    pub fn new(initial: SafetyState) -> Self {
        Self {
            state: Arc::new(RwLock::new(initial)),
        }
    }

    pub fn state(&self) -> SafetyState {
        self.state
            .read()
            .map(|state| *state)
            .unwrap_or(SafetyState::Killed)
    }

    pub fn set(&self, state: SafetyState) -> Result<(), SafetyStateStoreError> {
        let mut current = self
            .state
            .write()
            .map_err(|_| SafetyStateStoreError::LockPoisoned)?;
        *current = state;
        Ok(())
    }

    pub fn kill(&self) -> Result<SafetyState, SafetyStateStoreError> {
        self.set(SafetyState::Killed)?;
        Ok(SafetyState::Killed)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Error)]
pub enum RiskError {
    #[error("invalid account state: {0}")]
    InvalidAccount(String),
    #[error("invalid risk snapshot: {0}")]
    InvalidSnapshot(String),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct AccountState {
    pub cash: Decimal,
    pub available_cash: Decimal,
    pub equity: Decimal,
}

impl AccountState {
    pub fn new(cash: Decimal, available_cash: Decimal, equity: Decimal) -> Result<Self, RiskError> {
        if cash < Decimal::ZERO || available_cash < Decimal::ZERO || equity < Decimal::ZERO {
            return Err(RiskError::InvalidAccount(
                "cash, available cash, and equity must not be negative".to_owned(),
            ));
        }
        if available_cash > cash {
            return Err(RiskError::InvalidAccount(
                "available cash cannot exceed cash".to_owned(),
            ));
        }
        Ok(Self {
            cash,
            available_cash,
            equity,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct UnresolvedExecution {
    pub instrument: Instrument,
    pub action: TradeAction,
    pub client_order_id: String,
}

impl UnresolvedExecution {
    pub fn new(
        instrument: Instrument,
        action: TradeAction,
        client_order_id: impl Into<String>,
    ) -> Self {
        Self {
            instrument,
            action,
            client_order_id: client_order_id.into(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RiskSnapshot {
    pub evaluated_at: OffsetDateTime,
    pub account: AccountState,
    pub positions: Vec<PositionState>,
    pub market: Option<MarketContext>,
    pub mode: OperatingMode,
    pub safety_state: SafetyState,
    pub unresolved_executions: Vec<UnresolvedExecution>,
    pub protective_exit: bool,
    pub add_evidence_qualified: bool,
    pub session_loss: Decimal,
    pub session_drawdown: Decimal,
}

impl RiskSnapshot {
    pub fn new(
        evaluated_at: OffsetDateTime,
        account: AccountState,
        positions: Vec<PositionState>,
        mode: OperatingMode,
        safety_state: SafetyState,
    ) -> Result<Self, RiskError> {
        let mut seen = std::collections::BTreeSet::new();
        for position in &positions {
            position
                .validate()
                .map_err(|error| RiskError::InvalidSnapshot(error.to_string()))?;
            if !seen.insert(position.instrument().clone()) {
                return Err(RiskError::InvalidSnapshot(
                    "duplicate position instrument".to_owned(),
                ));
            }
        }
        Ok(Self {
            evaluated_at,
            account,
            positions,
            market: None,
            mode,
            safety_state,
            unresolved_executions: Vec::new(),
            protective_exit: false,
            add_evidence_qualified: false,
            session_loss: Decimal::ZERO,
            session_drawdown: Decimal::ZERO,
        })
    }

    pub fn with_positions(mut self, positions: Vec<PositionState>) -> Self {
        self.positions = positions;
        self
    }

    pub fn with_market(mut self, market: MarketContext) -> Self {
        self.market = Some(market);
        self
    }

    pub fn with_mode(mut self, mode: OperatingMode) -> Self {
        self.mode = mode;
        self
    }

    pub fn with_safety_state(mut self, state: SafetyState) -> Self {
        self.safety_state = state;
        self
    }

    pub fn with_unresolved_execution(mut self, execution: UnresolvedExecution) -> Self {
        self.unresolved_executions.push(execution);
        self
    }

    pub fn with_protective_exit(mut self, protective: bool) -> Self {
        self.protective_exit = protective;
        self
    }

    pub fn with_add_evidence_qualified(mut self, qualified: bool) -> Self {
        self.add_evidence_qualified = qualified;
        self
    }

    pub fn with_session_loss(mut self, value: Decimal) -> Self {
        self.session_loss = value;
        self
    }

    pub fn with_session_drawdown(mut self, value: Decimal) -> Self {
        self.session_drawdown = value;
        self
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RiskConfig {
    pub max_open_positions: usize,
    pub max_decision_age_secs: i64,
    pub max_trade_notional: Option<Decimal>,
    pub minimum_cash_reserve: Decimal,
    pub max_session_loss: Option<Decimal>,
    pub max_session_drawdown: Option<Decimal>,
}

impl Default for RiskConfig {
    fn default() -> Self {
        Self {
            max_open_positions: 3,
            max_decision_age_secs: 300,
            max_trade_notional: None,
            minimum_cash_reserve: Decimal::ZERO,
            max_session_loss: None,
            max_session_drawdown: None,
        }
    }
}

impl RiskConfig {
    pub fn with_max_open_positions(mut self, value: usize) -> Self {
        self.max_open_positions = value;
        self
    }

    pub fn with_max_decision_age_secs(mut self, value: i64) -> Self {
        self.max_decision_age_secs = value;
        self
    }

    pub fn with_max_trade_notional(mut self, value: Decimal) -> Self {
        self.max_trade_notional = Some(value);
        self
    }

    pub fn with_minimum_cash_reserve(mut self, value: Decimal) -> Self {
        self.minimum_cash_reserve = value;
        self
    }

    pub fn with_max_session_loss(mut self, value: Decimal) -> Self {
        self.max_session_loss = Some(value);
        self
    }

    pub fn with_max_session_drawdown(mut self, value: Decimal) -> Self {
        self.max_session_drawdown = Some(value);
        self
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum RiskOutcome {
    NoAction,
    Approved,
    ApprovedWithConstraints,
    Rejected,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum RiskReasonCode {
    InsufficientFunds,
    InsufficientInventory,
    PositionLimitReached,
    DecisionStale,
    ExecutionStateUnknown,
    TradingKilled,
    TradingSafeMode,
    InvalidInstrument,
    InvalidOrderIntent,
    LiveExposureExceeded,
    ConstraintApplied,
    AddWithoutNewEvidence,
    SessionLossExceeded,
    SessionDrawdownExceeded,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ApprovedIntent {
    pub decision_id: DecisionId,
    pub instrument: Instrument,
    pub action: TradeAction,
    pub operating_mode: OperatingMode,
    pub notional: Option<Decimal>,
    pub quantity: Option<Decimal>,
    pub protective: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RiskDecision {
    pub risk_decision_id: Uuid,
    pub decision_id: DecisionId,
    pub outcome: RiskOutcome,
    pub reasons: Vec<RiskReasonCode>,
    pub rules_evaluated: Vec<String>,
    pub evaluated_at: OffsetDateTime,
    pub approved_intent: Option<ApprovedIntent>,
}

impl RiskDecision {
    pub fn is_approved(&self) -> bool {
        matches!(
            self.outcome,
            RiskOutcome::Approved | RiskOutcome::ApprovedWithConstraints
        )
    }
}

pub struct RiskEngine {
    config: RiskConfig,
}

impl RiskEngine {
    pub fn new(config: RiskConfig) -> Self {
        Self { config }
    }

    pub fn evaluate(&self, decision: &TradeDecision, snapshot: &RiskSnapshot) -> RiskDecision {
        let mut reasons = Vec::new();
        let rules_evaluated = vec![
            "decision_validation".to_owned(),
            "freshness".to_owned(),
            "spot_inventory".to_owned(),
            "position_limit".to_owned(),
            "unresolved_execution".to_owned(),
            "safety_state".to_owned(),
            "capital_availability".to_owned(),
        ];
        let risk_decision_id = Uuid::new_v4();
        if decision.validate().is_err() || decision.instrument.validate().is_err() {
            reasons.push(RiskReasonCode::InvalidOrderIntent);
            return make_decision(
                risk_decision_id,
                decision,
                RiskOutcome::Rejected,
                reasons,
                rules_evaluated,
                snapshot.evaluated_at,
                None,
            );
        }
        if decision.action == TradeAction::Hold {
            return make_decision(
                risk_decision_id,
                decision,
                RiskOutcome::NoAction,
                reasons,
                rules_evaluated,
                snapshot.evaluated_at,
                None,
            );
        }

        let position = snapshot
            .positions
            .iter()
            .find(|position| position.instrument() == &decision.instrument);
        let protective = snapshot.protective_exit
            && decision.action == TradeAction::Sell
            && decision.desired_reduction_pct == Some(Decimal::ONE);
        let age = snapshot
            .evaluated_at
            .unix_timestamp()
            .saturating_sub(decision.created_at.unix_timestamp());
        if age > self.config.max_decision_age_secs && !protective {
            reasons.push(RiskReasonCode::DecisionStale);
        }

        match decision.action {
            TradeAction::Sell => {
                let Some(position) = position.filter(|position| position.is_open()) else {
                    reasons.push(RiskReasonCode::InsufficientInventory);
                    return make_decision(
                        risk_decision_id,
                        decision,
                        RiskOutcome::Rejected,
                        reasons,
                        rules_evaluated,
                        snapshot.evaluated_at,
                        None,
                    );
                };
                let reduction = decision.desired_reduction_pct.unwrap_or(Decimal::ZERO);
                let quantity = position_quantity(position).unwrap_or(Decimal::ZERO) * reduction;
                if quantity <= Decimal::ZERO
                    || quantity > position_quantity(position).unwrap_or(Decimal::ZERO)
                {
                    reasons.push(RiskReasonCode::InsufficientInventory);
                }
                if !reasons.is_empty() {
                    return make_decision(
                        risk_decision_id,
                        decision,
                        RiskOutcome::Rejected,
                        reasons,
                        rules_evaluated,
                        snapshot.evaluated_at,
                        None,
                    );
                }
                return make_decision(
                    risk_decision_id,
                    decision,
                    RiskOutcome::Approved,
                    reasons,
                    rules_evaluated,
                    snapshot.evaluated_at,
                    Some(ApprovedIntent {
                        decision_id: decision.decision_id,
                        instrument: decision.instrument.clone(),
                        action: TradeAction::Sell,
                        operating_mode: snapshot.mode,
                        notional: None,
                        quantity: Some(quantity),
                        protective,
                    }),
                );
            }
            TradeAction::Buy => {}
            TradeAction::Hold => unreachable!(),
        }

        let is_add = position.is_some_and(PositionState::is_open);
        if !protective {
            if self
                .config
                .max_session_loss
                .is_some_and(|limit| snapshot.session_loss >= limit)
            {
                reasons.push(RiskReasonCode::SessionLossExceeded);
            }
            if self
                .config
                .max_session_drawdown
                .is_some_and(|limit| snapshot.session_drawdown >= limit)
            {
                reasons.push(RiskReasonCode::SessionDrawdownExceeded);
            }
            if is_add && !snapshot.add_evidence_qualified {
                reasons.push(RiskReasonCode::AddWithoutNewEvidence);
            }
            if snapshot.safety_state == SafetyState::Killed {
                reasons.push(RiskReasonCode::TradingKilled);
            } else if snapshot.safety_state == SafetyState::Safe {
                reasons.push(RiskReasonCode::TradingSafeMode);
            }
            if snapshot.unresolved_executions.iter().any(|execution| {
                execution.instrument == decision.instrument && execution.action == TradeAction::Buy
            }) {
                reasons.push(RiskReasonCode::ExecutionStateUnknown);
            }
            if !is_add
                && snapshot
                    .positions
                    .iter()
                    .filter(|position| position.is_open())
                    .count()
                    >= self.config.max_open_positions
            {
                reasons.push(RiskReasonCode::PositionLimitReached);
            }
        }
        let requested = decision.requested_notional.unwrap_or(Decimal::ZERO);
        let available = snapshot.account.available_cash - self.config.minimum_cash_reserve;
        if available < requested {
            if snapshot.mode == OperatingMode::Live && available > Decimal::ZERO {
                reasons.push(RiskReasonCode::ConstraintApplied);
            } else {
                reasons.push(RiskReasonCode::InsufficientFunds);
            }
        }

        let mut approved_notional = requested;
        let mut constrained = false;
        if snapshot.mode == OperatingMode::Live {
            if let Some(limit) = self.config.max_trade_notional {
                if approved_notional > limit {
                    approved_notional = limit;
                    constrained = true;
                    reasons.push(RiskReasonCode::LiveExposureExceeded);
                    reasons.push(RiskReasonCode::ConstraintApplied);
                }
            }
            if approved_notional > available && available > Decimal::ZERO {
                approved_notional = available;
                constrained = true;
            }
        }
        let only_live_constraints = snapshot.mode == OperatingMode::Live
            && reasons.iter().all(|reason| {
                matches!(
                    reason,
                    RiskReasonCode::ConstraintApplied | RiskReasonCode::LiveExposureExceeded
                )
            });
        if !(reasons.is_empty() || only_live_constraints) {
            return make_decision(
                risk_decision_id,
                decision,
                RiskOutcome::Rejected,
                reasons,
                rules_evaluated,
                snapshot.evaluated_at,
                None,
            );
        }
        if approved_notional <= Decimal::ZERO {
            reasons.push(RiskReasonCode::InsufficientFunds);
            return make_decision(
                risk_decision_id,
                decision,
                RiskOutcome::Rejected,
                reasons,
                rules_evaluated,
                snapshot.evaluated_at,
                None,
            );
        }
        make_decision(
            risk_decision_id,
            decision,
            if constrained {
                RiskOutcome::ApprovedWithConstraints
            } else {
                RiskOutcome::Approved
            },
            reasons,
            rules_evaluated,
            snapshot.evaluated_at,
            Some(ApprovedIntent {
                decision_id: decision.decision_id,
                instrument: decision.instrument.clone(),
                action: TradeAction::Buy,
                operating_mode: snapshot.mode,
                notional: Some(approved_notional),
                quantity: None,
                protective: false,
            }),
        )
    }
}

fn make_decision(
    risk_decision_id: Uuid,
    decision: &TradeDecision,
    outcome: RiskOutcome,
    reasons: Vec<RiskReasonCode>,
    rules_evaluated: Vec<String>,
    evaluated_at: OffsetDateTime,
    approved_intent: Option<ApprovedIntent>,
) -> RiskDecision {
    RiskDecision {
        risk_decision_id,
        decision_id: decision.decision_id,
        outcome,
        reasons,
        rules_evaluated,
        evaluated_at,
        approved_intent,
    }
}

fn position_quantity(position: &PositionState) -> Option<Decimal> {
    match position {
        PositionState::Open { quantity, .. } => Some(*quantity),
        PositionState::Flat { .. } => None,
    }
}
