//! Deterministic, synthetic-capital spot execution and accounting.
//!
//! This module consumes canonical instruments and explicit market snapshots.
//! It has no Hermes, ATK, network, live-order, or risk-policy dependency.

use std::collections::{BTreeMap, BTreeSet};

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use time::OffsetDateTime;

use crate::domain::{Instrument, TradeAction};

fn basis_points() -> Decimal {
    Decimal::new(10_000, 0)
}

fn default_initial_cash() -> Decimal {
    Decimal::new(1_000, 0)
}

fn default_stop_loss_pct() -> Decimal {
    Decimal::new(15, 2)
}

fn default_take_profit_pct() -> Decimal {
    Decimal::new(30, 2)
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum SimulationError {
    #[error("invalid simulation configuration for {field}: {value}")]
    InvalidConfig { field: &'static str, value: Decimal },
    #[error("invalid market snapshot: {0}")]
    InvalidMarketSnapshot(&'static str),
    #[error("unsupported or invalid instrument: {0}")]
    UnsupportedInstrument(String),
    #[error("invalid order quantity: {0}")]
    InvalidQuantity(Decimal),
    #[error("invalid order notional: {0}")]
    InvalidNotional(Decimal),
    #[error("invalid price: {0}")]
    InvalidPrice(Decimal),
    #[error("HOLD cannot be submitted for execution")]
    HoldNotExecutable,
    #[error("insufficient synthetic cash: required {required}, available {available}")]
    InsufficientFunds {
        required: Decimal,
        available: Decimal,
    },
    #[error("insufficient inventory: requested {requested}, available {available}")]
    InsufficientInventory {
        requested: Decimal,
        available: Decimal,
    },
    #[error("order not found: {0}")]
    OrderNotFound(u64),
    #[error("order is already final: {0}")]
    OrderAlreadyFinal(u64),
    #[error("no market snapshot is available for {0}")]
    MissingMarketSnapshot(String),
    #[error("arithmetic overflow while calculating {0}")]
    ArithmeticFailure(&'static str),
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum FeeModel {
    None,
    FixedRate(Decimal),
}

impl FeeModel {
    fn validate(self) -> Result<(), SimulationError> {
        if let Self::FixedRate(rate) = self
            && (rate < Decimal::ZERO || rate > Decimal::ONE)
        {
            return Err(SimulationError::InvalidConfig {
                field: "fee_rate",
                value: rate,
            });
        }
        Ok(())
    }

    fn fee(self, gross_notional: Decimal) -> Result<Decimal, SimulationError> {
        match self {
            Self::None => Ok(Decimal::ZERO),
            Self::FixedRate(rate) => checked_mul(gross_notional, rate, "fee"),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum SlippageModel {
    None,
    FixedBps(Decimal),
}

impl SlippageModel {
    fn validate(self) -> Result<(), SimulationError> {
        if let Self::FixedBps(bps) = self
            && (bps < Decimal::ZERO || bps >= basis_points())
        {
            return Err(SimulationError::InvalidConfig {
                field: "slippage_bps",
                value: bps,
            });
        }
        Ok(())
    }

    fn adjusted_price(
        self,
        action: TradeAction,
        reference_price: Decimal,
    ) -> Result<(Decimal, Decimal), SimulationError> {
        let Self::FixedBps(bps) = self else {
            return Ok((reference_price, Decimal::ZERO));
        };
        let rate = checked_div(bps, basis_points(), "slippage rate")?;
        let factor = match action {
            TradeAction::Buy => checked_add(Decimal::ONE, rate, "buy slippage factor")?,
            TradeAction::Sell => checked_sub(Decimal::ONE, rate, "sell slippage factor")?,
            TradeAction::Hold => return Err(SimulationError::HoldNotExecutable),
        };
        let adjusted = checked_mul(reference_price, factor, "slippage-adjusted price")?;
        let slippage = match action {
            TradeAction::Buy => checked_sub(adjusted, reference_price, "buy slippage")?,
            TradeAction::Sell => checked_sub(reference_price, adjusted, "sell slippage")?,
            TradeAction::Hold => return Err(SimulationError::HoldNotExecutable),
        };
        Ok((adjusted, slippage))
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum FillModel {
    Full,
    Partial(Decimal),
    None,
}

impl FillModel {
    fn validate(self) -> Result<(), SimulationError> {
        if let Self::Partial(ratio) = self
            && (ratio <= Decimal::ZERO || ratio > Decimal::ONE)
        {
            return Err(SimulationError::InvalidConfig {
                field: "partial_fill_ratio",
                value: ratio,
            });
        }
        Ok(())
    }

    fn quantity(self, remaining: Decimal) -> Result<Option<Decimal>, SimulationError> {
        match self {
            Self::Full => Ok(Some(remaining)),
            Self::Partial(ratio) => Ok(Some(checked_mul(
                remaining,
                ratio,
                "partial fill quantity",
            )?)),
            Self::None => Ok(None),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct InitialPosition {
    pub instrument: Instrument,
    pub quantity: Decimal,
    pub average_entry_price: Decimal,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SimulationConfig {
    initial_cash: Decimal,
    initial_positions: Vec<InitialPosition>,
    fee_model: FeeModel,
    slippage_model: SlippageModel,
    fill_model: FillModel,
    stop_loss_pct: Decimal,
    take_profit_pct: Decimal,
}

impl SimulationConfig {
    pub fn new(initial_cash: Decimal) -> Self {
        Self {
            initial_cash,
            initial_positions: Vec::new(),
            fee_model: FeeModel::None,
            slippage_model: SlippageModel::None,
            fill_model: FillModel::Full,
            stop_loss_pct: default_stop_loss_pct(),
            take_profit_pct: default_take_profit_pct(),
        }
    }

    pub fn initial_cash(&self) -> Decimal {
        self.initial_cash
    }

    pub fn initial_positions(&self) -> &[InitialPosition] {
        &self.initial_positions
    }

    pub fn fee_model(&self) -> FeeModel {
        self.fee_model
    }

    pub fn slippage_model(&self) -> SlippageModel {
        self.slippage_model
    }

    pub fn fill_model(&self) -> FillModel {
        self.fill_model
    }

    pub fn stop_loss_pct(&self) -> Decimal {
        self.stop_loss_pct
    }

    pub fn take_profit_pct(&self) -> Decimal {
        self.take_profit_pct
    }

    pub fn with_initial_position(
        mut self,
        instrument: Instrument,
        quantity: Decimal,
        average_entry_price: Decimal,
    ) -> Self {
        self.initial_positions.push(InitialPosition {
            instrument,
            quantity,
            average_entry_price,
        });
        self
    }

    pub fn with_fee_model(mut self, fee_model: FeeModel) -> Self {
        self.fee_model = fee_model;
        self
    }

    pub fn with_fee_rate(self, rate: Decimal) -> Self {
        self.with_fee_model(FeeModel::FixedRate(rate))
    }

    pub fn with_slippage_model(mut self, slippage_model: SlippageModel) -> Self {
        self.slippage_model = slippage_model;
        self
    }

    pub fn with_slippage_bps(self, bps: Decimal) -> Self {
        self.with_slippage_model(SlippageModel::FixedBps(bps))
    }

    pub fn with_fill_model(mut self, fill_model: FillModel) -> Self {
        self.fill_model = fill_model;
        self
    }

    pub fn with_stop_loss_pct(mut self, stop_loss_pct: Decimal) -> Self {
        self.stop_loss_pct = stop_loss_pct;
        self
    }

    pub fn with_take_profit_pct(mut self, take_profit_pct: Decimal) -> Self {
        self.take_profit_pct = take_profit_pct;
        self
    }

    fn validate(&self) -> Result<(), SimulationError> {
        if self.initial_cash < Decimal::ZERO {
            return Err(SimulationError::InvalidConfig {
                field: "initial_cash",
                value: self.initial_cash,
            });
        }
        self.fee_model.validate()?;
        self.slippage_model.validate()?;
        self.fill_model.validate()?;
        if self.stop_loss_pct <= Decimal::ZERO || self.stop_loss_pct >= Decimal::ONE {
            return Err(SimulationError::InvalidConfig {
                field: "stop_loss_pct",
                value: self.stop_loss_pct,
            });
        }
        if self.take_profit_pct <= Decimal::ZERO {
            return Err(SimulationError::InvalidConfig {
                field: "take_profit_pct",
                value: self.take_profit_pct,
            });
        }

        let mut instruments = BTreeSet::new();
        for position in &self.initial_positions {
            position.instrument.validate().map_err(|_| {
                SimulationError::UnsupportedInstrument(position.instrument.to_string())
            })?;
            if !instruments.insert(position.instrument.clone()) {
                return Err(SimulationError::InvalidConfig {
                    field: "initial_positions",
                    value: Decimal::ZERO,
                });
            }
            if position.quantity <= Decimal::ZERO {
                return Err(SimulationError::InvalidConfig {
                    field: "initial_position.quantity",
                    value: position.quantity,
                });
            }
            if position.average_entry_price <= Decimal::ZERO {
                return Err(SimulationError::InvalidConfig {
                    field: "initial_position.average_entry_price",
                    value: position.average_entry_price,
                });
            }
        }
        Ok(())
    }
}

impl Default for SimulationConfig {
    fn default() -> Self {
        Self::new(default_initial_cash())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct MarketSnapshot {
    pub instrument: Instrument,
    pub timestamp: OffsetDateTime,
    pub best_bid: Decimal,
    pub best_ask: Decimal,
    pub last_price: Option<Decimal>,
}

impl MarketSnapshot {
    pub fn new(
        instrument: Instrument,
        timestamp: OffsetDateTime,
        best_bid: Decimal,
        best_ask: Decimal,
    ) -> Result<Self, SimulationError> {
        let snapshot = Self {
            instrument,
            timestamp,
            best_bid,
            best_ask,
            last_price: None,
        };
        snapshot.validate()?;
        Ok(snapshot)
    }

    pub fn with_last_price(mut self, last_price: Decimal) -> Result<Self, SimulationError> {
        self.last_price = Some(last_price);
        self.validate()?;
        Ok(self)
    }

    fn validate(&self) -> Result<(), SimulationError> {
        self.instrument
            .validate()
            .map_err(|_| SimulationError::UnsupportedInstrument(self.instrument.to_string()))?;
        if self.best_bid <= Decimal::ZERO || self.best_ask <= Decimal::ZERO {
            return Err(SimulationError::InvalidMarketSnapshot(
                "bid and ask must be positive",
            ));
        }
        if self.best_bid > self.best_ask {
            return Err(SimulationError::InvalidMarketSnapshot(
                "bid cannot exceed ask",
            ));
        }
        if self.last_price.is_some_and(|price| price <= Decimal::ZERO) {
            return Err(SimulationError::InvalidMarketSnapshot(
                "last price must be positive",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum OrderType {
    Market,
    Limit { price: Decimal },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum OrderSize {
    Quantity(Decimal),
    Notional(Decimal),
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ExecutionRequest {
    pub instrument: Instrument,
    pub action: TradeAction,
    pub size: OrderSize,
    pub order_type: OrderType,
    pub submitted_at: OffsetDateTime,
}

impl ExecutionRequest {
    pub fn market(
        instrument: Instrument,
        action: TradeAction,
        size: OrderSize,
        submitted_at: OffsetDateTime,
    ) -> Self {
        Self {
            instrument,
            action,
            size,
            order_type: OrderType::Market,
            submitted_at,
        }
    }

    pub fn limit(
        instrument: Instrument,
        action: TradeAction,
        size: OrderSize,
        price: Decimal,
        submitted_at: OffsetDateTime,
    ) -> Self {
        Self {
            instrument,
            action,
            size,
            order_type: OrderType::Limit { price },
            submitted_at,
        }
    }

    fn validate(&self) -> Result<(), SimulationError> {
        self.instrument
            .validate()
            .map_err(|_| SimulationError::UnsupportedInstrument(self.instrument.to_string()))?;
        if self.action == TradeAction::Hold {
            return Err(SimulationError::HoldNotExecutable);
        }
        match self.size {
            OrderSize::Quantity(quantity) if quantity > Decimal::ZERO => {}
            OrderSize::Quantity(quantity) => {
                return Err(SimulationError::InvalidQuantity(quantity));
            }
            OrderSize::Notional(notional) if notional > Decimal::ZERO => {}
            OrderSize::Notional(notional) => {
                return Err(SimulationError::InvalidNotional(notional));
            }
        }
        if let OrderType::Limit { price } = self.order_type
            && price <= Decimal::ZERO
        {
            return Err(SimulationError::InvalidPrice(price));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum OrderStatus {
    Pending,
    PartiallyFilled,
    Filled,
    Cancelled,
    Rejected,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum ExecutionReason {
    AgentOrder,
    HardStopLoss,
    HardTakeProfit,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SimulationFill {
    pub fill_id: u64,
    pub order_id: u64,
    pub instrument: Instrument,
    pub action: TradeAction,
    pub quantity: Decimal,
    pub reference_price: Decimal,
    pub price: Decimal,
    pub slippage: Decimal,
    pub gross_notional: Decimal,
    pub fee: Decimal,
    pub timestamp: OffsetDateTime,
    pub reason: ExecutionReason,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ExecutionResult {
    pub order_id: u64,
    pub status: OrderStatus,
    pub fill: Option<SimulationFill>,
    pub remaining_quantity: Option<Decimal>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SimulatedPosition {
    pub instrument: Instrument,
    pub quantity: Decimal,
    /// Effective weighted cost per unit, including entry fees.
    pub average_entry_price: Decimal,
    pub realized_pnl: Decimal,
}

impl SimulatedPosition {
    pub fn cost_basis(&self) -> Result<Decimal, SimulationError> {
        checked_mul(
            self.quantity,
            self.average_entry_price,
            "position cost basis",
        )
    }

    pub fn unrealized_pnl(&self, mark_price: Decimal) -> Result<Decimal, SimulationError> {
        if mark_price <= Decimal::ZERO {
            return Err(SimulationError::InvalidPrice(mark_price));
        }
        let market_value = checked_mul(self.quantity, mark_price, "unrealized market value")?;
        let cost_basis = self.cost_basis()?;
        checked_sub(market_value, cost_basis, "unrealized PnL")
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SimulatedOrder {
    pub order_id: u64,
    pub request: ExecutionRequest,
    pub status: OrderStatus,
    pub requested_quantity: Option<Decimal>,
    pub filled_quantity: Decimal,
    pub remaining_quantity: Option<Decimal>,
    pub fills: Vec<SimulationFill>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PortfolioSnapshot {
    pub cash: Decimal,
    pub positions: Vec<SimulatedPosition>,
    pub realized_pnl: Decimal,
    pub fees_paid: Decimal,
    pub equity: Decimal,
}

#[derive(Clone, Debug)]
struct SimulatedAccount {
    cash: Decimal,
    positions: BTreeMap<Instrument, SimulatedPosition>,
    realized_pnl: Decimal,
    fees_paid: Decimal,
}

impl SimulatedAccount {
    fn from_config(config: &SimulationConfig) -> Self {
        let positions = config
            .initial_positions
            .iter()
            .map(|position| {
                (
                    position.instrument.clone(),
                    SimulatedPosition {
                        instrument: position.instrument.clone(),
                        quantity: position.quantity,
                        average_entry_price: position.average_entry_price,
                        realized_pnl: Decimal::ZERO,
                    },
                )
            })
            .collect();
        Self {
            cash: config.initial_cash,
            positions,
            realized_pnl: Decimal::ZERO,
            fees_paid: Decimal::ZERO,
        }
    }

    fn apply_fill(&mut self, fill: &SimulationFill) -> Result<(), SimulationError> {
        match fill.action {
            TradeAction::Buy => {
                let total_cost = checked_add(fill.gross_notional, fill.fee, "buy cash impact")?;
                if total_cost > self.cash {
                    return Err(SimulationError::InsufficientFunds {
                        required: total_cost,
                        available: self.cash,
                    });
                }
                let new_cash = checked_sub(self.cash, total_cost, "buy cash balance")?;
                let new_fees_paid = checked_add(self.fees_paid, fill.fee, "fees paid")?;
                let existing = self.positions.get(&fill.instrument).cloned();
                let position = match existing {
                    Some(existing) => {
                        let old_cost = existing.cost_basis()?;
                        let new_cost =
                            checked_add(fill.gross_notional, fill.fee, "weighted entry cost")?;
                        let total_cost_basis = checked_add(old_cost, new_cost, "total cost basis")?;
                        let total_quantity =
                            checked_add(existing.quantity, fill.quantity, "position quantity")?;
                        let average_entry_price =
                            checked_div(total_cost_basis, total_quantity, "weighted entry price")?;
                        SimulatedPosition {
                            quantity: total_quantity,
                            average_entry_price,
                            ..existing
                        }
                    }
                    None => SimulatedPosition {
                        instrument: fill.instrument.clone(),
                        quantity: fill.quantity,
                        average_entry_price: checked_div(
                            checked_add(fill.gross_notional, fill.fee, "initial entry cost")?,
                            fill.quantity,
                            "initial entry price",
                        )?,
                        realized_pnl: Decimal::ZERO,
                    },
                };
                self.cash = new_cash;
                self.fees_paid = new_fees_paid;
                self.positions.insert(fill.instrument.clone(), position);
            }
            TradeAction::Sell => {
                let Some(existing) = self.positions.get(&fill.instrument).cloned() else {
                    return Err(SimulationError::InsufficientInventory {
                        requested: fill.quantity,
                        available: Decimal::ZERO,
                    });
                };
                if fill.quantity > existing.quantity {
                    return Err(SimulationError::InsufficientInventory {
                        requested: fill.quantity,
                        available: existing.quantity,
                    });
                }
                let proceeds = checked_sub(fill.gross_notional, fill.fee, "sell proceeds")?;
                let sold_cost = checked_mul(
                    existing.average_entry_price,
                    fill.quantity,
                    "sold cost basis",
                )?;
                let realized = checked_sub(proceeds, sold_cost, "realized PnL")?;
                let new_cash = checked_add(self.cash, proceeds, "sell cash balance")?;
                let new_realized_pnl = checked_add(self.realized_pnl, realized, "realized PnL")?;
                let new_fees_paid = checked_add(self.fees_paid, fill.fee, "fees paid")?;
                let remaining_quantity = checked_sub(
                    existing.quantity,
                    fill.quantity,
                    "remaining position quantity",
                )?;
                let remaining = if remaining_quantity.is_zero() {
                    None
                } else {
                    Some(SimulatedPosition {
                        quantity: remaining_quantity,
                        realized_pnl: checked_add(
                            existing.realized_pnl,
                            realized,
                            "position realized PnL",
                        )?,
                        ..existing
                    })
                };
                self.cash = new_cash;
                self.realized_pnl = new_realized_pnl;
                self.fees_paid = new_fees_paid;
                match remaining {
                    Some(remaining) => {
                        self.positions.insert(fill.instrument.clone(), remaining);
                    }
                    None => {
                        self.positions.remove(&fill.instrument);
                    }
                }
            }
            TradeAction::Hold => return Err(SimulationError::HoldNotExecutable),
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct SimulationEngine {
    config: SimulationConfig,
    account: SimulatedAccount,
    orders: BTreeMap<u64, SimulatedOrder>,
    latest_snapshots: BTreeMap<Instrument, MarketSnapshot>,
    next_order_id: u64,
    next_fill_id: u64,
}

impl SimulationEngine {
    pub fn new(config: SimulationConfig) -> Result<Self, SimulationError> {
        config.validate()?;
        Ok(Self {
            account: SimulatedAccount::from_config(&config),
            config,
            orders: BTreeMap::new(),
            latest_snapshots: BTreeMap::new(),
            next_order_id: 1,
            next_fill_id: 1,
        })
    }

    pub fn cash(&self) -> Decimal {
        self.account.cash
    }

    pub fn realized_pnl(&self) -> Decimal {
        self.account.realized_pnl
    }

    pub fn fees_paid(&self) -> Decimal {
        self.account.fees_paid
    }

    pub fn position(&self, instrument: &Instrument) -> Option<&SimulatedPosition> {
        self.account.positions.get(instrument)
    }

    pub fn positions(&self) -> Vec<SimulatedPosition> {
        self.account.positions.values().cloned().collect()
    }

    pub fn order(&self, order_id: u64) -> Option<&SimulatedOrder> {
        self.orders.get(&order_id)
    }

    pub fn pending_orders(&self) -> Vec<SimulatedOrder> {
        self.orders
            .values()
            .filter(|order| {
                matches!(
                    order.status,
                    OrderStatus::Pending | OrderStatus::PartiallyFilled
                )
            })
            .cloned()
            .collect()
    }

    pub fn execute(
        &mut self,
        request: ExecutionRequest,
        snapshot: MarketSnapshot,
    ) -> Result<ExecutionResult, SimulationError> {
        request.validate()?;
        snapshot.validate()?;
        if request.instrument != snapshot.instrument {
            return Err(SimulationError::InvalidMarketSnapshot(
                "request and snapshot instruments must match",
            ));
        }
        self.latest_snapshots
            .insert(snapshot.instrument.clone(), snapshot.clone());

        let order_id = self.allocate_order_id()?;
        let mut order = SimulatedOrder {
            requested_quantity: initial_quantity(&request.size),
            filled_quantity: Decimal::ZERO,
            remaining_quantity: initial_quantity(&request.size),
            order_id,
            request,
            status: OrderStatus::Pending,
            fills: Vec::new(),
        };
        let result = match order.request.order_type {
            OrderType::Market => self.fill_order(
                &mut order,
                &snapshot,
                ExecutionReason::AgentOrder,
                self.config.fill_model,
            )?,
            OrderType::Limit { .. } => {
                if self.limit_is_fillable(&order.request, &snapshot)? {
                    self.fill_order(
                        &mut order,
                        &snapshot,
                        ExecutionReason::AgentOrder,
                        self.config.fill_model,
                    )?
                } else {
                    ExecutionResult {
                        order_id,
                        status: OrderStatus::Pending,
                        fill: None,
                        remaining_quantity: order.remaining_quantity,
                    }
                }
            }
        };
        self.orders.insert(order_id, order);
        Ok(result)
    }

    pub fn cancel_order(&mut self, order_id: u64) -> Result<(), SimulationError> {
        let order = self
            .orders
            .get_mut(&order_id)
            .ok_or(SimulationError::OrderNotFound(order_id))?;
        if matches!(order.status, OrderStatus::Filled | OrderStatus::Cancelled) {
            return Err(SimulationError::OrderAlreadyFinal(order_id));
        }
        order.status = OrderStatus::Cancelled;
        Ok(())
    }

    pub fn on_market_snapshot(
        &mut self,
        snapshot: MarketSnapshot,
    ) -> Result<Vec<ExecutionResult>, SimulationError> {
        snapshot.validate()?;
        self.latest_snapshots
            .insert(snapshot.instrument.clone(), snapshot.clone());
        let mut results = Vec::new();

        let protective_reason = self.protective_reason(&snapshot)?;
        if protective_reason.is_some() {
            let pending_ids: Vec<u64> = self
                .orders
                .values()
                .filter(|order| {
                    order.request.instrument == snapshot.instrument
                        && matches!(
                            order.status,
                            OrderStatus::Pending | OrderStatus::PartiallyFilled
                        )
                        && matches!(order.request.order_type, OrderType::Limit { .. })
                })
                .map(|order| order.order_id)
                .collect();
            for order_id in pending_ids {
                if let Some(order) = self.orders.get_mut(&order_id) {
                    order.status = OrderStatus::Cancelled;
                }
            }
        }

        if let Some(reason) = protective_reason
            && let Some(position) = self.position(&snapshot.instrument).cloned()
        {
            let request = ExecutionRequest::market(
                snapshot.instrument.clone(),
                TradeAction::Sell,
                OrderSize::Quantity(position.quantity),
                snapshot.timestamp,
            );
            let order_id = self.allocate_order_id()?;
            let mut order = SimulatedOrder {
                order_id,
                request,
                status: OrderStatus::Pending,
                requested_quantity: Some(position.quantity),
                filled_quantity: Decimal::ZERO,
                remaining_quantity: Some(position.quantity),
                fills: Vec::new(),
            };
            let result = self.fill_order(&mut order, &snapshot, reason, FillModel::Full)?;
            self.orders.insert(order_id, order);
            results.push(result);
        }

        let order_ids: Vec<u64> = self
            .orders
            .values()
            .filter(|order| {
                order.request.instrument == snapshot.instrument
                    && matches!(
                        order.status,
                        OrderStatus::Pending | OrderStatus::PartiallyFilled
                    )
                    && matches!(order.request.order_type, OrderType::Limit { .. })
            })
            .map(|order| order.order_id)
            .collect();
        for order_id in order_ids {
            let mut order = self
                .orders
                .remove(&order_id)
                .ok_or(SimulationError::OrderNotFound(order_id))?;
            if self.limit_is_fillable(&order.request, &snapshot)? {
                match self.fill_order(
                    &mut order,
                    &snapshot,
                    ExecutionReason::AgentOrder,
                    self.config.fill_model,
                ) {
                    Ok(result) => results.push(result),
                    Err(error) => {
                        self.orders.insert(order_id, order);
                        return Err(error);
                    }
                }
            }
            self.orders.insert(order_id, order);
        }
        Ok(results)
    }

    pub fn portfolio_snapshot(&self) -> Result<PortfolioSnapshot, SimulationError> {
        let mut equity = self.account.cash;
        let mut positions = Vec::with_capacity(self.account.positions.len());
        for (instrument, position) in &self.account.positions {
            let snapshot = self
                .latest_snapshots
                .get(instrument)
                .ok_or_else(|| SimulationError::MissingMarketSnapshot(instrument.to_string()))?;
            equity = checked_add(
                equity,
                checked_mul(position.quantity, snapshot.best_bid, "marked inventory")?,
                "portfolio equity",
            )?;
            positions.push(position.clone());
        }
        Ok(PortfolioSnapshot {
            cash: self.account.cash,
            positions,
            realized_pnl: self.account.realized_pnl,
            fees_paid: self.account.fees_paid,
            equity,
        })
    }

    fn allocate_order_id(&mut self) -> Result<u64, SimulationError> {
        let order_id = self.next_order_id;
        self.next_order_id = self
            .next_order_id
            .checked_add(1)
            .ok_or(SimulationError::ArithmeticFailure("order ID"))?;
        Ok(order_id)
    }

    fn allocate_fill_id(&mut self) -> Result<u64, SimulationError> {
        let fill_id = self.next_fill_id;
        self.next_fill_id = self
            .next_fill_id
            .checked_add(1)
            .ok_or(SimulationError::ArithmeticFailure("fill ID"))?;
        Ok(fill_id)
    }

    fn limit_is_fillable(
        &self,
        request: &ExecutionRequest,
        snapshot: &MarketSnapshot,
    ) -> Result<bool, SimulationError> {
        let OrderType::Limit { price: limit } = request.order_type else {
            return Ok(true);
        };
        let reference = reference_price(request.action, snapshot);
        let (effective, _) = self
            .config
            .slippage_model
            .adjusted_price(request.action, reference)?;
        Ok(match request.action {
            TradeAction::Buy => reference <= limit && effective <= limit,
            TradeAction::Sell => reference >= limit && effective >= limit,
            TradeAction::Hold => false,
        })
    }

    fn fill_order(
        &mut self,
        order: &mut SimulatedOrder,
        snapshot: &MarketSnapshot,
        reason: ExecutionReason,
        fill_model: FillModel,
    ) -> Result<ExecutionResult, SimulationError> {
        let reference_price = reference_price(order.request.action, snapshot);
        let (price, slippage) = self
            .config
            .slippage_model
            .adjusted_price(order.request.action, reference_price)?;
        if price <= Decimal::ZERO {
            return Err(SimulationError::InvalidPrice(price));
        }
        if let OrderType::Limit { price: limit } = order.request.order_type {
            let fillable = match order.request.action {
                TradeAction::Buy => price <= limit,
                TradeAction::Sell => price >= limit,
                TradeAction::Hold => false,
            };
            if !fillable {
                return Ok(ExecutionResult {
                    order_id: order.order_id,
                    status: order.status,
                    fill: None,
                    remaining_quantity: order.remaining_quantity,
                });
            }
        }

        let requested_quantity = match order.requested_quantity {
            Some(quantity) => quantity,
            None => {
                let quantity = match order.request.size {
                    OrderSize::Quantity(quantity) => quantity,
                    OrderSize::Notional(notional) => {
                        checked_div(notional, price, "notional quantity")?
                    }
                };
                order.requested_quantity = Some(quantity);
                order.remaining_quantity = Some(quantity);
                quantity
            }
        };
        let remaining = order.remaining_quantity.unwrap_or(requested_quantity);
        if remaining <= Decimal::ZERO {
            order.status = OrderStatus::Filled;
            return Ok(ExecutionResult {
                order_id: order.order_id,
                status: order.status,
                fill: None,
                remaining_quantity: Some(Decimal::ZERO),
            });
        }
        self.ensure_full_fill_possible(
            order.request.action,
            &order.request.instrument,
            remaining,
            price,
        )?;
        let Some(fill_quantity) = fill_model.quantity(remaining)? else {
            return Ok(ExecutionResult {
                order_id: order.order_id,
                status: order.status,
                fill: None,
                remaining_quantity: Some(remaining),
            });
        };
        if fill_quantity <= Decimal::ZERO {
            return Err(SimulationError::InvalidQuantity(fill_quantity));
        }
        let gross_notional = checked_mul(fill_quantity, price, "gross notional")?;
        let fee = self.config.fee_model.fee(gross_notional)?;
        let fill = SimulationFill {
            fill_id: self.allocate_fill_id()?,
            order_id: order.order_id,
            instrument: order.request.instrument.clone(),
            action: order.request.action,
            quantity: fill_quantity,
            reference_price,
            price,
            slippage,
            gross_notional,
            fee,
            timestamp: snapshot.timestamp,
            reason,
        };
        self.account.apply_fill(&fill)?;
        order.filled_quantity =
            checked_add(order.filled_quantity, fill_quantity, "filled quantity")?;
        let remaining_quantity = checked_sub(remaining, fill_quantity, "remaining quantity")?;
        order.remaining_quantity = Some(remaining_quantity);
        order.fills.push(fill.clone());
        order.status = if remaining_quantity.is_zero() {
            OrderStatus::Filled
        } else {
            OrderStatus::PartiallyFilled
        };
        Ok(ExecutionResult {
            order_id: order.order_id,
            status: order.status,
            fill: Some(fill),
            remaining_quantity: Some(remaining_quantity),
        })
    }

    fn ensure_full_fill_possible(
        &self,
        action: TradeAction,
        instrument: &Instrument,
        quantity: Decimal,
        price: Decimal,
    ) -> Result<(), SimulationError> {
        match action {
            TradeAction::Buy => {
                let gross = checked_mul(quantity, price, "buy affordability")?;
                let fee = self.config.fee_model.fee(gross)?;
                let total = checked_add(gross, fee, "buy affordability")?;
                if total > self.account.cash {
                    return Err(SimulationError::InsufficientFunds {
                        required: total,
                        available: self.account.cash,
                    });
                }
            }
            TradeAction::Sell => {
                let available = self
                    .account
                    .positions
                    .get(instrument)
                    .map(|position| position.quantity);
                let available = available.unwrap_or(Decimal::ZERO);
                if quantity > available {
                    return Err(SimulationError::InsufficientInventory {
                        requested: quantity,
                        available,
                    });
                }
            }
            TradeAction::Hold => return Err(SimulationError::HoldNotExecutable),
        }
        Ok(())
    }

    fn protective_reason(
        &self,
        snapshot: &MarketSnapshot,
    ) -> Result<Option<ExecutionReason>, SimulationError> {
        let Some(position) = self.position(&snapshot.instrument) else {
            return Ok(None);
        };
        let stop_factor = checked_sub(Decimal::ONE, self.config.stop_loss_pct, "stop-loss factor")?;
        let stop_price = checked_mul(
            position.average_entry_price,
            stop_factor,
            "stop-loss threshold",
        )?;
        let profit_factor = checked_add(
            Decimal::ONE,
            self.config.take_profit_pct,
            "take-profit factor",
        )?;
        let take_profit_price = checked_mul(
            position.average_entry_price,
            profit_factor,
            "take-profit threshold",
        )?;
        if snapshot.best_bid <= stop_price {
            Ok(Some(ExecutionReason::HardStopLoss))
        } else if snapshot.best_bid >= take_profit_price {
            Ok(Some(ExecutionReason::HardTakeProfit))
        } else {
            Ok(None)
        }
    }
}

fn initial_quantity(size: &OrderSize) -> Option<Decimal> {
    match size {
        OrderSize::Quantity(quantity) => Some(*quantity),
        OrderSize::Notional(_) => None,
    }
}

fn reference_price(action: TradeAction, snapshot: &MarketSnapshot) -> Decimal {
    match action {
        TradeAction::Buy => snapshot.best_ask,
        TradeAction::Sell => snapshot.best_bid,
        TradeAction::Hold => Decimal::ZERO,
    }
}

fn checked_add(
    left: Decimal,
    right: Decimal,
    operation: &'static str,
) -> Result<Decimal, SimulationError> {
    left.checked_add(right)
        .ok_or(SimulationError::ArithmeticFailure(operation))
}

fn checked_sub(
    left: Decimal,
    right: Decimal,
    operation: &'static str,
) -> Result<Decimal, SimulationError> {
    left.checked_sub(right)
        .ok_or(SimulationError::ArithmeticFailure(operation))
}

fn checked_mul(
    left: Decimal,
    right: Decimal,
    operation: &'static str,
) -> Result<Decimal, SimulationError> {
    left.checked_mul(right)
        .ok_or(SimulationError::ArithmeticFailure(operation))
}

fn checked_div(
    left: Decimal,
    right: Decimal,
    operation: &'static str,
) -> Result<Decimal, SimulationError> {
    left.checked_div(right)
        .ok_or(SimulationError::ArithmeticFailure(operation))
}
