//! Risk-approved execution planning, lifecycle state, and reconciliation.

use std::collections::BTreeMap;

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::atk::{
    AtkClient, AtkHealth, CancellationOutcome, McpTransport, OrderQuery, OrderState, SizeUnit,
    SpotOrderRequest, TradingMode,
};
use crate::domain::{DecisionId, Instrument, TradeAction, TradeDecision, Urgency};
use crate::risk::{ApprovedIntent, OperatingMode, RiskDecision};
use crate::simulation::{ExecutionRequest, OrderSize, SimulationEngine, SimulationError};

#[derive(Clone, Debug, Eq, PartialEq, Error)]
pub enum ExecutionError {
    #[error("risk decision is not approved")]
    RiskNotApproved,
    #[error("risk decision and execution plan do not match")]
    RiskPlanMismatch,
    #[error("market price constraint prevents safe execution")]
    PriceConstraint,
    #[error("invalid execution plan: {0}")]
    InvalidPlan(String),
    #[error("execution gateway is unavailable")]
    GatewayUnavailable,
    #[error("execution gateway error: {0}")]
    Gateway(String),
    #[error("unresolved execution prevents duplicate exposure")]
    UnresolvedDuplicate,
    #[error("execution requires reconciliation before new risk")]
    ReconciliationRequired,
    #[error("execution backend mode does not match approved mode")]
    EnvironmentMismatch,
    #[error("client order ID is already associated with another execution")]
    DuplicateClientOrderId,
    #[error("execution record not found: {0}")]
    NotFound(Uuid),
    #[error("limit order is not stale")]
    NotStale,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct MarketQuote {
    pub instrument: Instrument,
    pub observed_at: OffsetDateTime,
    pub bid: Decimal,
    pub ask: Decimal,
}

impl MarketQuote {
    pub fn new(
        instrument: Instrument,
        observed_at: OffsetDateTime,
        bid: Decimal,
        ask: Decimal,
    ) -> Result<Self, ExecutionError> {
        if bid <= Decimal::ZERO || ask <= Decimal::ZERO || bid > ask {
            return Err(ExecutionError::InvalidPlan(
                "market quote must have positive bid <= ask".to_owned(),
            ));
        }
        Ok(Self {
            instrument,
            observed_at,
            bid,
            ask,
        })
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum PlannedOrderType {
    Market,
    Limit { price: Decimal },
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum PlannedSize {
    Quantity(Decimal),
    Notional(Decimal),
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ExecutionPlan {
    pub execution_id: Uuid,
    pub decision_id: DecisionId,
    pub risk_decision_id: Uuid,
    pub instrument: Instrument,
    pub action: TradeAction,
    pub operating_mode: OperatingMode,
    pub size: PlannedSize,
    pub order_type: PlannedOrderType,
    pub client_order_id: String,
    pub created_at: OffsetDateTime,
    pub risk_increasing: bool,
    pub protective: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum ExecutionState {
    Planned,
    Submitted,
    Acknowledged,
    PartiallyFilled,
    Filled,
    CancelPending,
    Cancelled,
    Rejected,
    Unknown,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GatewayPlacement {
    Acknowledged { exchange_order_id: String },
    Rejected { code: String, message: String },
    Unknown { reason: String },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GatewayOrderState {
    Open,
    PartiallyFilled,
    Filled,
    Cancelled,
    Rejected,
    Unknown,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GatewayOrderStatus {
    pub state: GatewayOrderState,
    pub filled_quantity: Decimal,
    pub remaining_quantity: Option<Decimal>,
    pub average_price: Option<Decimal>,
}

impl GatewayOrderStatus {
    pub fn filled(quantity: Decimal, average_price: Decimal) -> Self {
        Self {
            state: GatewayOrderState::Filled,
            filled_quantity: quantity,
            remaining_quantity: Some(Decimal::ZERO),
            average_price: Some(average_price),
        }
    }
}

pub trait ExecutionGateway {
    fn place(&mut self, plan: &ExecutionPlan) -> GatewayPlacement;
    fn query(
        &mut self,
        instrument: &Instrument,
        client_order_id: &str,
    ) -> Result<GatewayOrderStatus, ExecutionError>;
    fn cancel(&mut self, instrument: &Instrument, client_order_id: &str) -> GatewayPlacement;
    fn is_available(&self) -> bool;

    fn operating_mode(&self) -> OperatingMode {
        OperatingMode::Simulation
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ExecutionReceipt {
    pub execution_id: Uuid,
    pub client_order_id: String,
    pub exchange_order_id: Option<String>,
    pub state: ExecutionState,
    pub filled_quantity: Decimal,
    pub remaining_quantity: Option<Decimal>,
    pub average_price: Option<Decimal>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ExecutionCheckpoint {
    pub plan: ExecutionPlan,
    pub receipt: ExecutionReceipt,
}

#[derive(Clone, Debug)]
struct ExecutionRecord {
    plan: ExecutionPlan,
    receipt: ExecutionReceipt,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ExecutionPolicy {
    pub limit_order_timeout_secs: i64,
    pub max_market_spread_bps: Decimal,
}

impl Default for ExecutionPolicy {
    fn default() -> Self {
        Self {
            limit_order_timeout_secs: 60,
            max_market_spread_bps: Decimal::from(200),
        }
    }
}

impl ExecutionPolicy {
    pub fn with_limit_order_timeout_secs(mut self, value: i64) -> Self {
        self.limit_order_timeout_secs = value;
        self
    }

    pub fn plan(
        &self,
        risk: &RiskDecision,
        decision: &TradeDecision,
        quote: &MarketQuote,
    ) -> Result<ExecutionPlan, ExecutionError> {
        decision
            .validate()
            .map_err(|error| ExecutionError::InvalidPlan(error.to_string()))?;
        if !risk.is_approved() {
            return Err(ExecutionError::RiskNotApproved);
        }
        let intent = risk
            .approved_intent
            .as_ref()
            .ok_or(ExecutionError::RiskNotApproved)?;
        if intent.instrument != decision.instrument || quote.instrument != decision.instrument {
            return Err(ExecutionError::RiskPlanMismatch);
        }
        let order_type = if decision.urgency == Urgency::High {
            let spread_bps = (quote.ask - quote.bid) * Decimal::from(10_000) / quote.ask;
            if spread_bps > self.max_market_spread_bps {
                return Err(ExecutionError::PriceConstraint);
            }
            if decision.action == TradeAction::Buy
                && decision
                    .max_acceptable_price
                    .is_some_and(|price| price < quote.ask)
            {
                return Err(ExecutionError::PriceConstraint);
            }
            PlannedOrderType::Market
        } else {
            let price = decision
                .max_acceptable_price
                .unwrap_or(match decision.action {
                    TradeAction::Buy => quote.ask,
                    TradeAction::Sell => quote.bid,
                    TradeAction::Hold => return Err(ExecutionError::RiskNotApproved),
                });
            PlannedOrderType::Limit { price }
        };
        let size = match intent {
            ApprovedIntent {
                action: TradeAction::Buy,
                notional: Some(notional),
                ..
            } => PlannedSize::Notional(*notional),
            ApprovedIntent {
                action: TradeAction::Sell,
                quantity: Some(quantity),
                ..
            } => PlannedSize::Quantity(*quantity),
            _ => {
                return Err(ExecutionError::InvalidPlan(
                    "approved size is missing".to_owned(),
                ));
            }
        };
        let execution_id = Uuid::new_v4();
        Ok(ExecutionPlan {
            execution_id,
            decision_id: decision.decision_id,
            risk_decision_id: risk.risk_decision_id,
            instrument: decision.instrument.clone(),
            action: decision.action,
            operating_mode: intent.operating_mode,
            size,
            order_type,
            client_order_id: format!("pt-{}", execution_id.simple()),
            created_at: quote.observed_at,
            risk_increasing: decision.action == TradeAction::Buy,
            protective: intent.protective,
        })
    }
}

pub struct ExecutionEngine<G> {
    gateway: G,
    records: BTreeMap<Uuid, ExecutionRecord>,
    reconciliation_complete: bool,
}

impl<G: ExecutionGateway> ExecutionEngine<G> {
    pub fn new(gateway: G) -> Self {
        Self {
            gateway,
            records: BTreeMap::new(),
            reconciliation_complete: true,
        }
    }

    pub fn restore(gateway: G, checkpoints: impl IntoIterator<Item = ExecutionCheckpoint>) -> Self {
        let records = checkpoints
            .into_iter()
            .map(|checkpoint| {
                (
                    checkpoint.plan.execution_id,
                    ExecutionRecord {
                        plan: checkpoint.plan,
                        receipt: checkpoint.receipt,
                    },
                )
            })
            .collect();
        Self {
            gateway,
            records,
            reconciliation_complete: false,
        }
    }

    pub fn checkpoints(&self) -> Vec<ExecutionCheckpoint> {
        self.records
            .values()
            .map(|record| ExecutionCheckpoint {
                plan: record.plan.clone(),
                receipt: record.receipt.clone(),
            })
            .collect()
    }

    pub fn gateway(&self) -> &G {
        &self.gateway
    }

    pub fn gateway_mut(&mut self) -> &mut G {
        &mut self.gateway
    }

    pub fn can_accept_new_risk(&self) -> bool {
        self.reconciliation_complete
            && !self.records.values().any(|record| {
                record.receipt.state == ExecutionState::Unknown && record.plan.risk_increasing
            })
    }

    pub fn execute(
        &mut self,
        risk: &RiskDecision,
        plan: ExecutionPlan,
    ) -> Result<ExecutionReceipt, ExecutionError> {
        if !risk.is_approved() {
            return Err(ExecutionError::RiskNotApproved);
        }
        if risk.risk_decision_id != plan.risk_decision_id || risk.decision_id != plan.decision_id {
            return Err(ExecutionError::RiskPlanMismatch);
        }
        let Some(intent) = risk.approved_intent.as_ref() else {
            return Err(ExecutionError::RiskNotApproved);
        };
        let plan_matches_intent = intent.instrument == plan.instrument
            && intent.action == plan.action
            && intent.operating_mode == plan.operating_mode
            && intent.protective == plan.protective
            && match (intent.notional, intent.quantity, plan.size) {
                (Some(notional), None, PlannedSize::Notional(actual)) => notional == actual,
                (None, Some(quantity), PlannedSize::Quantity(actual)) => quantity == actual,
                _ => false,
            };
        if !plan_matches_intent {
            return Err(ExecutionError::RiskPlanMismatch);
        }
        if plan.risk_increasing && !self.can_accept_new_risk() {
            return Err(ExecutionError::ReconciliationRequired);
        }
        if self.gateway.operating_mode() != plan.operating_mode {
            return Err(ExecutionError::EnvironmentMismatch);
        }
        if !self.gateway.is_available() {
            self.reconciliation_complete = false;
            return Err(ExecutionError::GatewayUnavailable);
        }
        if self.records.values().any(|record| {
            record.receipt.state == ExecutionState::Unknown
                && record.plan.instrument == plan.instrument
                && record.plan.risk_increasing
        }) {
            return Err(ExecutionError::UnresolvedDuplicate);
        }
        if self.records.values().any(|record| {
            record.receipt.client_order_id == plan.client_order_id
                && record.plan.execution_id != plan.execution_id
        }) {
            return Err(ExecutionError::DuplicateClientOrderId);
        }
        if let Some(existing) = self.records.get(&plan.execution_id) {
            if existing.receipt.state == ExecutionState::Unknown {
                return Err(ExecutionError::UnresolvedDuplicate);
            }
            return Ok(existing.receipt.clone());
        }

        let mut record = ExecutionRecord {
            receipt: ExecutionReceipt {
                execution_id: plan.execution_id,
                client_order_id: plan.client_order_id.clone(),
                exchange_order_id: None,
                state: ExecutionState::Submitted,
                filled_quantity: Decimal::ZERO,
                remaining_quantity: match plan.size {
                    PlannedSize::Quantity(quantity) => Some(quantity),
                    PlannedSize::Notional(_) => None,
                },
                average_price: None,
            },
            plan,
        };
        let placement = self.gateway.place(&record.plan);
        match placement {
            GatewayPlacement::Acknowledged { exchange_order_id } => {
                record.receipt.state = ExecutionState::Acknowledged;
                record.receipt.exchange_order_id = Some(exchange_order_id);
            }
            GatewayPlacement::Rejected { .. } => {
                record.receipt.state = ExecutionState::Rejected;
            }
            GatewayPlacement::Unknown { .. } => {
                record.receipt.state = ExecutionState::Unknown;
                self.reconciliation_complete = false;
            }
        }
        let receipt = record.receipt.clone();
        self.records.insert(record.plan.execution_id, record);
        Ok(receipt)
    }

    pub fn reconcile(&mut self, execution_id: Uuid) -> Result<ExecutionReceipt, ExecutionError> {
        let record = self
            .records
            .get(&execution_id)
            .cloned()
            .ok_or(ExecutionError::NotFound(execution_id))?;
        if matches!(
            record.receipt.state,
            ExecutionState::Filled | ExecutionState::Cancelled | ExecutionState::Rejected
        ) {
            return Ok(record.receipt);
        }
        let status = self
            .gateway
            .query(&record.plan.instrument, &record.plan.client_order_id)?;
        let receipt = {
            let current = self
                .records
                .get_mut(&execution_id)
                .ok_or(ExecutionError::NotFound(execution_id))?;
            current.receipt.filled_quantity = status.filled_quantity;
            current.receipt.remaining_quantity = status.remaining_quantity;
            current.receipt.average_price = status.average_price;
            current.receipt.state = match status.state {
                GatewayOrderState::Open => ExecutionState::Acknowledged,
                GatewayOrderState::PartiallyFilled => ExecutionState::PartiallyFilled,
                GatewayOrderState::Filled => ExecutionState::Filled,
                GatewayOrderState::Cancelled => ExecutionState::Cancelled,
                GatewayOrderState::Rejected => ExecutionState::Rejected,
                GatewayOrderState::Unknown => ExecutionState::Unknown,
            };
            current.receipt.clone()
        };
        self.reconciliation_complete = !self
            .records
            .values()
            .any(|item| item.receipt.state == ExecutionState::Unknown && item.plan.risk_increasing);
        Ok(receipt)
    }

    pub fn expire_limit(
        &mut self,
        execution_id: Uuid,
        now: OffsetDateTime,
        policy: &ExecutionPolicy,
    ) -> Result<ExecutionReceipt, ExecutionError> {
        let record = self
            .records
            .get(&execution_id)
            .cloned()
            .ok_or(ExecutionError::NotFound(execution_id))?;
        if !matches!(record.plan.order_type, PlannedOrderType::Limit { .. })
            || now.unix_timestamp() - record.plan.created_at.unix_timestamp()
                < policy.limit_order_timeout_secs
        {
            return Err(ExecutionError::NotStale);
        }
        if matches!(
            record.receipt.state,
            ExecutionState::Filled | ExecutionState::Cancelled
        ) {
            return Ok(record.receipt);
        }
        if !self.gateway.is_available() {
            self.reconciliation_complete = false;
            return Err(ExecutionError::GatewayUnavailable);
        }
        if matches!(
            self.gateway
                .cancel(&record.plan.instrument, &record.plan.client_order_id),
            GatewayPlacement::Unknown { .. }
        ) {
            if let Some(current) = self.records.get_mut(&execution_id) {
                current.receipt.state = ExecutionState::Unknown;
            }
            self.reconciliation_complete = false;
            return self
                .records
                .get(&execution_id)
                .map(|item| item.receipt.clone())
                .ok_or(ExecutionError::NotFound(execution_id));
        }
        if let Some(current) = self.records.get_mut(&execution_id) {
            current.receipt.state = ExecutionState::CancelPending;
        }
        self.reconcile(execution_id)
    }

    pub fn startup_reconcile(&mut self) -> Result<bool, ExecutionError> {
        self.reconciliation_complete = false;
        let ids: Vec<Uuid> = self
            .records
            .values()
            .filter(|record| {
                !matches!(
                    record.receipt.state,
                    ExecutionState::Filled | ExecutionState::Cancelled | ExecutionState::Rejected
                )
            })
            .map(|record| record.plan.execution_id)
            .collect();
        for id in ids {
            self.reconcile(id)?;
        }
        self.reconciliation_complete = self.records.values().all(|record| {
            !matches!(
                record.receipt.state,
                ExecutionState::Unknown | ExecutionState::CancelPending
            )
        });
        Ok(self.reconciliation_complete)
    }
}

impl<T: McpTransport> ExecutionGateway for AtkClient<T> {
    fn place(&mut self, plan: &ExecutionPlan) -> GatewayPlacement {
        let result = match (plan.order_type, plan.size) {
            (PlannedOrderType::Market, PlannedSize::Notional(size)) => SpotOrderRequest::market(
                plan.instrument.clone(),
                plan.action.into(),
                size,
                SizeUnit::Quote,
                plan.client_order_id.clone(),
            ),
            (PlannedOrderType::Market, PlannedSize::Quantity(size)) => SpotOrderRequest::market(
                plan.instrument.clone(),
                plan.action.into(),
                size,
                SizeUnit::Base,
                plan.client_order_id.clone(),
            ),
            (PlannedOrderType::Limit { price }, PlannedSize::Notional(size)) => {
                SpotOrderRequest::limit(
                    plan.instrument.clone(),
                    plan.action.into(),
                    size,
                    SizeUnit::Quote,
                    price,
                    plan.client_order_id.clone(),
                )
            }
            (PlannedOrderType::Limit { price }, PlannedSize::Quantity(size)) => {
                SpotOrderRequest::limit(
                    plan.instrument.clone(),
                    plan.action.into(),
                    size,
                    SizeUnit::Base,
                    price,
                    plan.client_order_id.clone(),
                )
            }
        };
        let Ok(request) = result else {
            return GatewayPlacement::Unknown {
                reason: "invalid ATK order request".to_owned(),
            };
        };
        match self.place_spot_order(&request) {
            Ok(crate::atk::PlacementOutcome::Confirmed { order_id, .. }) => {
                GatewayPlacement::Acknowledged {
                    exchange_order_id: order_id,
                }
            }
            Ok(crate::atk::PlacementOutcome::Rejected { code, message }) => {
                GatewayPlacement::Rejected { code, message }
            }
            Ok(crate::atk::PlacementOutcome::Unknown { reason, .. }) => {
                GatewayPlacement::Unknown { reason }
            }
            Err(error) => GatewayPlacement::Unknown {
                reason: error.to_string(),
            },
        }
    }

    fn query(
        &mut self,
        instrument: &Instrument,
        client_order_id: &str,
    ) -> Result<GatewayOrderStatus, ExecutionError> {
        let status = self
            .query_order(&OrderQuery::by_client_id(
                instrument.clone(),
                client_order_id,
            ))
            .map_err(|error| ExecutionError::Gateway(error.to_string()))?;
        Ok(GatewayOrderStatus {
            state: match status.state {
                OrderState::Open => GatewayOrderState::Open,
                OrderState::PartiallyFilled => GatewayOrderState::PartiallyFilled,
                OrderState::Filled => GatewayOrderState::Filled,
                OrderState::Canceled => GatewayOrderState::Cancelled,
                OrderState::Failed => GatewayOrderState::Rejected,
                OrderState::Unknown(_) => GatewayOrderState::Unknown,
            },
            filled_quantity: status.filled_quantity,
            remaining_quantity: None,
            average_price: status.average_price,
        })
    }

    fn cancel(&mut self, instrument: &Instrument, client_order_id: &str) -> GatewayPlacement {
        match self.cancel_order(&crate::atk::CancelOrderRequest::by_client_id(
            instrument.clone(),
            client_order_id,
        )) {
            Ok(CancellationOutcome::Confirmed { order_id, .. }) => GatewayPlacement::Acknowledged {
                exchange_order_id: order_id,
            },
            Ok(CancellationOutcome::Rejected { code, message }) => {
                GatewayPlacement::Rejected { code, message }
            }
            Ok(CancellationOutcome::Unknown { reason, .. }) => GatewayPlacement::Unknown { reason },
            Err(error) => GatewayPlacement::Unknown {
                reason: error.to_string(),
            },
        }
    }

    fn is_available(&self) -> bool {
        self.health().status == AtkHealth::Ready && self.capabilities().execution_ready
    }

    fn operating_mode(&self) -> OperatingMode {
        match self.trading_mode() {
            TradingMode::Demo => OperatingMode::Demo,
            TradingMode::Live => OperatingMode::Live,
        }
    }
}

impl From<TradeAction> for crate::atk::OrderSide {
    fn from(action: TradeAction) -> Self {
        match action {
            TradeAction::Buy => Self::Buy,
            TradeAction::Sell => Self::Sell,
            TradeAction::Hold => Self::Buy,
        }
    }
}

pub struct SimulationExecutor {
    engine: SimulationEngine,
}

impl SimulationExecutor {
    pub fn new(engine: SimulationEngine) -> Self {
        Self { engine }
    }

    pub fn engine(&self) -> &SimulationEngine {
        &self.engine
    }

    pub fn engine_mut(&mut self) -> &mut SimulationEngine {
        &mut self.engine
    }

    pub fn execute(
        &mut self,
        plan: &ExecutionPlan,
        snapshot: crate::simulation::MarketSnapshot,
    ) -> Result<crate::simulation::ExecutionResult, SimulationError> {
        let size = match plan.size {
            PlannedSize::Quantity(value) => OrderSize::Quantity(value),
            PlannedSize::Notional(value) => OrderSize::Notional(value),
        };
        let request = match plan.order_type {
            PlannedOrderType::Market => ExecutionRequest::market(
                plan.instrument.clone(),
                plan.action,
                size,
                snapshot.timestamp,
            ),
            PlannedOrderType::Limit { price } => ExecutionRequest::limit(
                plan.instrument.clone(),
                plan.action,
                size,
                price,
                snapshot.timestamp,
            ),
        };
        self.engine.execute(request, snapshot)
    }
}
