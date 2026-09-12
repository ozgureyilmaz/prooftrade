use prooftrade::domain::{Instrument, TradeAction};
use prooftrade::simulation::{
    ExecutionReason, ExecutionRequest, FeeModel, FillModel, MarketSnapshot, OrderSize, OrderStatus,
    SimulationConfig, SimulationEngine, SimulationError, SlippageModel,
};
use rust_decimal::Decimal;
use time::OffsetDateTime;

fn cash(value: i64) -> Decimal {
    Decimal::new(value, 0)
}

fn timestamp(seconds: i64) -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(seconds).unwrap()
}

fn btc() -> Instrument {
    Instrument::parse("BTC-USDT").unwrap()
}

fn instrument(symbol: &str) -> Instrument {
    Instrument::parse(symbol).unwrap()
}

fn market(bid: i64, ask: i64) -> MarketSnapshot {
    market_at(bid, ask, 1_700_000_000)
}

fn market_at(bid: i64, ask: i64, seconds: i64) -> MarketSnapshot {
    MarketSnapshot::new(btc(), timestamp(seconds), cash(bid), cash(ask)).unwrap()
}

fn snapshot_for(symbol: &str, bid: i64, ask: i64, seconds: i64) -> MarketSnapshot {
    MarketSnapshot::new(instrument(symbol), timestamp(seconds), cash(bid), cash(ask)).unwrap()
}

#[test]
fn new_simulation_account_starts_with_configured_cash() {
    let engine = SimulationEngine::new(SimulationConfig::new(cash(1_000))).unwrap();

    assert_eq!(engine.cash(), cash(1_000));
}

#[test]
fn explicit_thirty_usdt_simulation_is_supported() {
    let engine = SimulationEngine::new(SimulationConfig::new(cash(30))).unwrap();

    assert_eq!(engine.cash(), cash(30));
}

#[test]
fn negative_starting_cash_is_rejected() {
    let error = SimulationEngine::new(SimulationConfig::new(Decimal::new(-1, 0))).unwrap_err();

    assert!(matches!(error, SimulationError::InvalidConfig { .. }));
}

#[test]
fn market_buy_uses_ask_and_updates_cash_and_inventory() {
    let mut engine = SimulationEngine::new(SimulationConfig::new(cash(1_000))).unwrap();
    let request = ExecutionRequest::market(
        btc(),
        TradeAction::Buy,
        OrderSize::Quantity(cash(2)),
        timestamp(1_700_000_000),
    );

    let result = engine.execute(request, market(99, 100)).unwrap();

    assert_eq!(result.status, OrderStatus::Filled);
    assert_eq!(result.fill.unwrap().price, cash(100));
    assert_eq!(engine.cash(), cash(800));
    assert_eq!(engine.position(&btc()).unwrap().quantity, cash(2));
}

#[test]
fn market_sell_uses_bid_and_updates_cash_and_inventory() {
    let mut engine = SimulationEngine::new(SimulationConfig::new(cash(1_000))).unwrap();
    let buy = ExecutionRequest::market(
        btc(),
        TradeAction::Buy,
        OrderSize::Quantity(cash(2)),
        timestamp(1_700_000_000),
    );
    engine.execute(buy, market(99, 100)).unwrap();
    let sell = ExecutionRequest::market(
        btc(),
        TradeAction::Sell,
        OrderSize::Quantity(cash(1)),
        timestamp(1_700_000_001),
    );

    let result = engine.execute(sell, market(109, 110)).unwrap();

    assert_eq!(result.fill.unwrap().price, cash(109));
    assert_eq!(engine.cash(), cash(909));
    assert_eq!(engine.position(&btc()).unwrap().quantity, cash(1));
}

#[test]
fn market_buy_beyond_cash_is_rejected_without_mutation() {
    let mut engine = SimulationEngine::new(SimulationConfig::new(cash(100))).unwrap();
    let request = ExecutionRequest::market(
        btc(),
        TradeAction::Buy,
        OrderSize::Quantity(cash(2)),
        timestamp(1_700_000_000),
    );

    let error = engine.execute(request, market(99, 100)).unwrap_err();

    assert!(matches!(error, SimulationError::InsufficientFunds { .. }));
    assert_eq!(engine.cash(), cash(100));
    assert!(engine.position(&btc()).is_none());
    assert_eq!(engine.fees_paid(), Decimal::ZERO);
}

#[test]
fn market_sell_beyond_inventory_is_rejected_without_mutation() {
    let mut engine = SimulationEngine::new(SimulationConfig::new(cash(1_000))).unwrap();
    let buy = ExecutionRequest::market(
        btc(),
        TradeAction::Buy,
        OrderSize::Quantity(cash(1)),
        timestamp(1_700_000_000),
    );
    engine.execute(buy, market(99, 100)).unwrap();
    let sell = ExecutionRequest::market(
        btc(),
        TradeAction::Sell,
        OrderSize::Quantity(cash(2)),
        timestamp(1_700_000_001),
    );

    let error = engine.execute(sell, market(109, 110)).unwrap_err();

    assert!(matches!(
        error,
        SimulationError::InsufficientInventory { .. }
    ));
    assert_eq!(engine.position(&btc()).unwrap().quantity, cash(1));
    assert_eq!(engine.cash(), cash(900));
}

#[test]
fn hold_is_not_an_executable_simulation_request() {
    let mut engine = SimulationEngine::new(SimulationConfig::new(cash(1_000))).unwrap();
    let request = ExecutionRequest::market(
        btc(),
        TradeAction::Hold,
        OrderSize::Quantity(cash(1)),
        timestamp(1_700_000_000),
    );

    let error = engine.execute(request, market(99, 100)).unwrap_err();

    assert!(matches!(error, SimulationError::HoldNotExecutable));
    assert_eq!(engine.cash(), cash(1_000));
}

#[test]
fn market_orders_apply_adverse_slippage_and_fees_to_accounting() {
    let config = SimulationConfig::new(cash(1_000))
        .with_slippage_model(SlippageModel::FixedBps(cash(100)))
        .with_fee_model(FeeModel::FixedRate(Decimal::new(1, 3)));
    let mut engine = SimulationEngine::new(config).unwrap();
    let request = ExecutionRequest::market(
        btc(),
        TradeAction::Buy,
        OrderSize::Quantity(cash(2)),
        timestamp(1_700_000_000),
    );

    let result = engine.execute(request, market(99, 100)).unwrap();
    let fill = result.fill.unwrap();

    assert_eq!(fill.reference_price, cash(100));
    assert_eq!(fill.price, Decimal::new(101, 0));
    assert_eq!(fill.slippage, cash(1));
    assert_eq!(fill.gross_notional, cash(202));
    assert_eq!(fill.fee, Decimal::new(202, 3));
    assert_eq!(engine.cash(), Decimal::new(797_798, 3));
    assert_eq!(engine.fees_paid(), Decimal::new(202, 3));
}

#[test]
fn notional_buy_is_converted_to_quantity_at_the_fill_price() {
    let mut engine = SimulationEngine::new(SimulationConfig::new(cash(1_000))).unwrap();
    let request = ExecutionRequest::market(
        btc(),
        TradeAction::Buy,
        OrderSize::Notional(cash(100)),
        timestamp(1_700_000_000),
    );

    engine.execute(request, market(99, 100)).unwrap();

    assert_eq!(engine.position(&btc()).unwrap().quantity, cash(1));
    assert_eq!(engine.cash(), cash(900));
}

#[test]
fn limit_buy_remains_pending_until_ask_reaches_the_limit() {
    let mut engine = SimulationEngine::new(SimulationConfig::new(cash(1_000))).unwrap();
    let request = ExecutionRequest::limit(
        btc(),
        TradeAction::Buy,
        OrderSize::Quantity(cash(1)),
        cash(100),
        timestamp(1_700_000_000),
    );

    let submitted = engine.execute(request, market(99, 101)).unwrap();
    assert_eq!(submitted.status, OrderStatus::Pending);
    assert!(submitted.fill.is_none());
    assert_eq!(engine.cash(), cash(1_000));

    let fills = engine
        .on_market_snapshot(market_at(99, 100, 1_700_000_001))
        .unwrap();

    assert_eq!(fills.len(), 1);
    assert_eq!(fills[0].status, OrderStatus::Filled);
    assert_eq!(engine.position(&btc()).unwrap().quantity, cash(1));
}

#[test]
fn limit_sell_remains_pending_until_bid_reaches_the_limit() {
    let config =
        SimulationConfig::new(Decimal::ZERO).with_initial_position(btc(), cash(1), cash(100));
    let mut engine = SimulationEngine::new(config).unwrap();
    let request = ExecutionRequest::limit(
        btc(),
        TradeAction::Sell,
        OrderSize::Quantity(cash(1)),
        cash(110),
        timestamp(1_700_000_000),
    );

    let submitted = engine.execute(request, market(109, 111)).unwrap();
    assert_eq!(submitted.status, OrderStatus::Pending);

    let fills = engine
        .on_market_snapshot(market_at(110, 111, 1_700_000_001))
        .unwrap();

    assert_eq!(fills[0].fill.as_ref().unwrap().price, cash(110));
    assert!(engine.position(&btc()).is_none());
}

#[test]
fn cancelled_limit_order_cannot_fill_later() {
    let mut engine = SimulationEngine::new(SimulationConfig::new(cash(1_000))).unwrap();
    let request = ExecutionRequest::limit(
        btc(),
        TradeAction::Buy,
        OrderSize::Quantity(cash(1)),
        cash(100),
        timestamp(1_700_000_000),
    );
    let submitted = engine.execute(request, market(99, 101)).unwrap();

    engine.cancel_order(submitted.order_id).unwrap();
    let fills = engine
        .on_market_snapshot(market_at(99, 100, 1_700_000_001))
        .unwrap();

    assert!(fills.is_empty());
    assert_eq!(
        engine.order(submitted.order_id).unwrap().status,
        OrderStatus::Cancelled
    );
    assert!(engine.position(&btc()).is_none());
}

#[test]
fn partial_market_fill_keeps_the_remainder_explicit() {
    let config =
        SimulationConfig::new(cash(1_000)).with_fill_model(FillModel::Partial(Decimal::new(50, 2)));
    let mut engine = SimulationEngine::new(config).unwrap();
    let request = ExecutionRequest::market(
        btc(),
        TradeAction::Buy,
        OrderSize::Quantity(cash(4)),
        timestamp(1_700_000_000),
    );

    let result = engine.execute(request, market(99, 100)).unwrap();

    assert_eq!(result.status, OrderStatus::PartiallyFilled);
    assert_eq!(result.fill.unwrap().quantity, cash(2));
    assert_eq!(result.remaining_quantity, Some(cash(2)));
    assert_eq!(engine.order(result.order_id).unwrap().fills.len(), 1);
    assert_eq!(engine.cash(), cash(800));
}

#[test]
fn second_buy_updates_weighted_average_entry_price() {
    let mut engine = SimulationEngine::new(SimulationConfig::new(cash(1_000))).unwrap();
    let first = ExecutionRequest::market(
        btc(),
        TradeAction::Buy,
        OrderSize::Quantity(cash(1)),
        timestamp(1_700_000_000),
    );
    engine.execute(first, market(99, 100)).unwrap();
    let second = ExecutionRequest::market(
        btc(),
        TradeAction::Buy,
        OrderSize::Quantity(cash(1)),
        timestamp(1_700_000_001),
    );

    engine
        .execute(second, market_at(119, 120, 1_700_000_001))
        .unwrap();

    let position = engine.position(&btc()).unwrap();
    assert_eq!(position.quantity, cash(2));
    assert_eq!(position.average_entry_price, cash(110));
}

#[test]
fn partial_sell_realizes_pnl_and_preserves_remaining_cost_basis() {
    let mut engine = SimulationEngine::new(SimulationConfig::new(cash(1_000))).unwrap();
    let buy = ExecutionRequest::market(
        btc(),
        TradeAction::Buy,
        OrderSize::Quantity(cash(2)),
        timestamp(1_700_000_000),
    );
    engine.execute(buy, market(99, 100)).unwrap();
    let sell = ExecutionRequest::market(
        btc(),
        TradeAction::Sell,
        OrderSize::Quantity(Decimal::new(5, 1)),
        timestamp(1_700_000_001),
    );

    engine
        .execute(sell, market_at(129, 130, 1_700_000_001))
        .unwrap();

    let position = engine.position(&btc()).unwrap();
    assert_eq!(position.quantity, Decimal::new(15, 1));
    assert_eq!(position.average_entry_price, cash(100));
    assert_eq!(engine.realized_pnl(), Decimal::new(145, 1));
}

#[test]
fn portfolio_equity_marks_inventory_at_the_conservative_bid() {
    let mut engine = SimulationEngine::new(SimulationConfig::new(cash(1_000))).unwrap();
    let buy = ExecutionRequest::market(
        btc(),
        TradeAction::Buy,
        OrderSize::Quantity(cash(1)),
        timestamp(1_700_000_000),
    );
    engine.execute(buy, market(99, 100)).unwrap();

    let portfolio = engine.portfolio_snapshot().unwrap();

    assert_eq!(portfolio.cash, cash(900));
    assert_eq!(portfolio.equity, cash(999));
}

#[test]
fn hard_stop_forces_full_exit_at_current_bid() {
    let config =
        SimulationConfig::new(Decimal::ZERO).with_initial_position(btc(), cash(1), cash(100));
    let mut engine = SimulationEngine::new(config).unwrap();

    let results = engine
        .on_market_snapshot(market_at(82, 83, 1_700_000_001))
        .unwrap();

    assert_eq!(results.len(), 1);
    assert_eq!(
        results[0].fill.as_ref().unwrap().reason,
        ExecutionReason::HardStopLoss
    );
    assert_eq!(results[0].fill.as_ref().unwrap().price, cash(82));
    assert!(engine.position(&btc()).is_none());
}

#[test]
fn hard_stop_cancels_pending_conflicting_limit_orders_before_exit() {
    let config =
        SimulationConfig::new(cash(1_000)).with_initial_position(btc(), cash(1), cash(100));
    let mut engine = SimulationEngine::new(config).unwrap();
    let pending_buy = ExecutionRequest::limit(
        btc(),
        TradeAction::Buy,
        OrderSize::Quantity(cash(1)),
        cash(90),
        timestamp(1_700_000_000),
    );
    let pending = engine
        .execute(pending_buy, market_at(85, 95, 1_700_000_000))
        .unwrap();
    assert_eq!(pending.status, OrderStatus::Pending);

    let exits = engine
        .on_market_snapshot(market_at(82, 83, 1_700_000_001))
        .unwrap();

    assert_eq!(exits.len(), 1);
    assert_eq!(
        exits[0].fill.as_ref().unwrap().reason,
        ExecutionReason::HardStopLoss
    );
    assert_eq!(
        engine.order(pending.order_id).unwrap().status,
        OrderStatus::Cancelled
    );
    assert!(engine.position(&btc()).is_none());
}

#[test]
fn hard_take_profit_forces_full_exit_without_agent_input() {
    let config =
        SimulationConfig::new(Decimal::ZERO).with_initial_position(btc(), cash(1), cash(100));
    let mut engine = SimulationEngine::new(config).unwrap();

    let results = engine
        .on_market_snapshot(market_at(130, 131, 1_700_000_001))
        .unwrap();

    assert_eq!(results.len(), 1);
    assert_eq!(
        results[0].fill.as_ref().unwrap().reason,
        ExecutionReason::HardTakeProfit
    );
    assert_eq!(results[0].fill.as_ref().unwrap().price, cash(130));
    assert!(engine.position(&btc()).is_none());
}

#[test]
fn hard_stop_uses_slippage_and_fees_on_a_gap_down() {
    let config = SimulationConfig::new(Decimal::ZERO)
        .with_initial_position(btc(), cash(1), cash(100))
        .with_slippage_model(SlippageModel::FixedBps(cash(100)))
        .with_fee_model(FeeModel::FixedRate(Decimal::new(1, 3)));
    let mut engine = SimulationEngine::new(config).unwrap();

    let results = engine
        .on_market_snapshot(market_at(82, 83, 1_700_000_001))
        .unwrap();
    let fill = results[0].fill.as_ref().unwrap();

    assert_eq!(fill.price, Decimal::new(8_118, 2));
    assert_eq!(fill.fee, Decimal::new(8_118, 5));
    assert_eq!(engine.cash(), Decimal::new(8_109_882, 5));
}

#[test]
fn default_protective_thresholds_are_fifteen_and_thirty_percent() {
    let config = SimulationConfig::default();

    assert_eq!(config.initial_cash(), cash(1_000));
    assert_eq!(config.stop_loss_pct(), Decimal::new(15, 2));
    assert_eq!(config.take_profit_pct(), Decimal::new(30, 2));
}

#[test]
fn negative_fee_rate_is_rejected() {
    let config =
        SimulationConfig::new(cash(1_000)).with_fee_model(FeeModel::FixedRate(Decimal::new(-1, 3)));

    let error = SimulationEngine::new(config).unwrap_err();

    assert!(matches!(
        error,
        SimulationError::InvalidConfig {
            field: "fee_rate",
            ..
        }
    ));
}

#[test]
fn negative_slippage_is_rejected() {
    let config = SimulationConfig::new(cash(1_000))
        .with_slippage_model(SlippageModel::FixedBps(Decimal::new(-1, 0)));

    let error = SimulationEngine::new(config).unwrap_err();

    assert!(matches!(
        error,
        SimulationError::InvalidConfig {
            field: "slippage_bps",
            ..
        }
    ));
}

#[test]
fn zero_partial_fill_ratio_is_rejected() {
    let config =
        SimulationConfig::new(cash(1_000)).with_fill_model(FillModel::Partial(Decimal::ZERO));

    let error = SimulationEngine::new(config).unwrap_err();

    assert!(matches!(
        error,
        SimulationError::InvalidConfig {
            field: "partial_fill_ratio",
            ..
        }
    ));
}

#[test]
fn zero_stop_loss_is_rejected() {
    let config = SimulationConfig::new(cash(1_000)).with_stop_loss_pct(Decimal::ZERO);

    let error = SimulationEngine::new(config).unwrap_err();

    assert!(matches!(
        error,
        SimulationError::InvalidConfig {
            field: "stop_loss_pct",
            ..
        }
    ));
}

#[test]
fn zero_take_profit_is_rejected() {
    let config = SimulationConfig::new(cash(1_000)).with_take_profit_pct(Decimal::ZERO);

    let error = SimulationEngine::new(config).unwrap_err();

    assert!(matches!(
        error,
        SimulationError::InvalidConfig {
            field: "take_profit_pct",
            ..
        }
    ));
}

#[test]
fn crossed_market_snapshot_is_rejected() {
    let error =
        MarketSnapshot::new(btc(), timestamp(1_700_000_000), cash(101), cash(100)).unwrap_err();

    assert!(matches!(
        error,
        SimulationError::InvalidMarketSnapshot("bid cannot exceed ask")
    ));
}

#[test]
fn zero_order_quantity_is_rejected_without_mutation() {
    let mut engine = SimulationEngine::new(SimulationConfig::new(cash(1_000))).unwrap();
    let request = ExecutionRequest::market(
        btc(),
        TradeAction::Buy,
        OrderSize::Quantity(Decimal::ZERO),
        timestamp(1_700_000_000),
    );

    let error = engine.execute(request, market(99, 100)).unwrap_err();

    assert!(matches!(error, SimulationError::InvalidQuantity(value) if value.is_zero()));
    assert_eq!(engine.cash(), cash(1_000));
    assert!(engine.order(1).is_none());
}

#[test]
fn limit_price_must_be_positive() {
    let mut engine = SimulationEngine::new(SimulationConfig::new(cash(1_000))).unwrap();
    let request = ExecutionRequest::limit(
        btc(),
        TradeAction::Buy,
        OrderSize::Quantity(cash(1)),
        Decimal::ZERO,
        timestamp(1_700_000_000),
    );

    let error = engine.execute(request, market(99, 100)).unwrap_err();

    assert!(matches!(error, SimulationError::InvalidPrice(value) if value.is_zero()));
}

#[test]
fn request_and_market_snapshot_must_use_the_same_instrument() {
    let mut engine = SimulationEngine::new(SimulationConfig::new(cash(1_000))).unwrap();
    let request = ExecutionRequest::market(
        btc(),
        TradeAction::Buy,
        OrderSize::Quantity(cash(1)),
        timestamp(1_700_000_000),
    );

    let error = engine
        .execute(request, snapshot_for("ETH-USDT", 99, 100, 1_700_000_000))
        .unwrap_err();

    assert!(matches!(
        error,
        SimulationError::InvalidMarketSnapshot("request and snapshot instruments must match")
    ));
    assert_eq!(engine.cash(), cash(1_000));
}

#[test]
fn multiple_instruments_have_independent_positions() {
    let mut engine = SimulationEngine::new(SimulationConfig::new(cash(1_000))).unwrap();
    let btc_buy = ExecutionRequest::market(
        btc(),
        TradeAction::Buy,
        OrderSize::Quantity(cash(1)),
        timestamp(1_700_000_000),
    );
    engine.execute(btc_buy, market(99, 100)).unwrap();
    let eth_buy = ExecutionRequest::market(
        instrument("ETH-USDT"),
        TradeAction::Buy,
        OrderSize::Quantity(cash(2)),
        timestamp(1_700_000_001),
    );

    engine
        .execute(eth_buy, snapshot_for("ETH-USDT", 49, 50, 1_700_000_001))
        .unwrap();

    assert_eq!(engine.position(&btc()).unwrap().quantity, cash(1));
    assert_eq!(
        engine.position(&instrument("ETH-USDT")).unwrap().quantity,
        cash(2)
    );
    assert_eq!(engine.positions().len(), 2);
}

#[test]
fn partial_limit_fill_leaves_remainder_for_a_later_market_event() {
    let config =
        SimulationConfig::new(cash(1_000)).with_fill_model(FillModel::Partial(Decimal::new(50, 2)));
    let mut engine = SimulationEngine::new(config).unwrap();
    let request = ExecutionRequest::limit(
        btc(),
        TradeAction::Buy,
        OrderSize::Quantity(cash(4)),
        cash(100),
        timestamp(1_700_000_000),
    );
    let submitted = engine.execute(request, market(99, 101)).unwrap();

    let first = engine
        .on_market_snapshot(market_at(99, 100, 1_700_000_001))
        .unwrap();
    let second = engine
        .on_market_snapshot(market_at(99, 100, 1_700_000_002))
        .unwrap();

    assert_eq!(first[0].status, OrderStatus::PartiallyFilled);
    assert_eq!(first[0].remaining_quantity, Some(cash(2)));
    assert_eq!(second[0].fill.as_ref().unwrap().quantity, cash(1));
    assert_eq!(engine.order(submitted.order_id).unwrap().fills.len(), 2);
    assert_eq!(engine.position(&btc()).unwrap().quantity, cash(3));
}

#[test]
fn portfolio_snapshot_requires_a_mark_for_each_open_position() {
    let config =
        SimulationConfig::new(Decimal::ZERO).with_initial_position(btc(), cash(1), cash(100));
    let engine = SimulationEngine::new(config).unwrap();

    let error = engine.portfolio_snapshot().unwrap_err();

    assert!(matches!(error, SimulationError::MissingMarketSnapshot(_)));
}

#[test]
fn identical_simulation_inputs_produce_identical_results() {
    fn run() -> (ExecutionResultForTest, PortfolioForTest) {
        let mut engine = SimulationEngine::new(
            SimulationConfig::new(cash(1_000))
                .with_fee_rate(Decimal::new(1, 3))
                .with_slippage_bps(cash(10)),
        )
        .unwrap();
        let request = ExecutionRequest::market(
            btc(),
            TradeAction::Buy,
            OrderSize::Quantity(cash(2)),
            timestamp(1_700_000_000),
        );
        let result = engine.execute(request, market(99, 100)).unwrap();
        let portfolio = engine.portfolio_snapshot().unwrap();
        (
            ExecutionResultForTest::from(result),
            PortfolioForTest::from(portfolio),
        )
    }

    assert_eq!(run(), run());
}

#[derive(Debug, Eq, PartialEq)]
struct ExecutionResultForTest {
    status: OrderStatus,
    fill: Option<(Decimal, Decimal, Decimal)>,
}

impl From<prooftrade::simulation::ExecutionResult> for ExecutionResultForTest {
    fn from(value: prooftrade::simulation::ExecutionResult) -> Self {
        Self {
            status: value.status,
            fill: value.fill.map(|fill| (fill.quantity, fill.price, fill.fee)),
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
struct PortfolioForTest {
    cash: Decimal,
    equity: Decimal,
    positions: Vec<(Instrument, Decimal, Decimal)>,
}

impl From<prooftrade::simulation::PortfolioSnapshot> for PortfolioForTest {
    fn from(value: prooftrade::simulation::PortfolioSnapshot) -> Self {
        Self {
            cash: value.cash,
            equity: value.equity,
            positions: value
                .positions
                .into_iter()
                .map(|position| {
                    (
                        position.instrument,
                        position.quantity,
                        position.average_entry_price,
                    )
                })
                .collect(),
        }
    }
}

#[test]
fn fees_are_included_in_realized_pnl() {
    let config = SimulationConfig::new(cash(1_000)).with_fee_rate(Decimal::new(1, 3));
    let mut engine = SimulationEngine::new(config).unwrap();
    let buy = ExecutionRequest::market(
        btc(),
        TradeAction::Buy,
        OrderSize::Quantity(cash(1)),
        timestamp(1_700_000_000),
    );
    engine.execute(buy, market(99, 100)).unwrap();
    let sell = ExecutionRequest::market(
        btc(),
        TradeAction::Sell,
        OrderSize::Quantity(cash(1)),
        timestamp(1_700_000_001),
    );

    engine
        .execute(sell, market_at(110, 111, 1_700_000_001))
        .unwrap();

    assert_eq!(engine.realized_pnl(), Decimal::new(979, 2));
    assert_eq!(engine.cash(), Decimal::new(100_979, 2));
}

#[test]
fn position_stays_open_before_protective_thresholds() {
    let config =
        SimulationConfig::new(Decimal::ZERO).with_initial_position(btc(), cash(1), cash(100));
    let mut engine = SimulationEngine::new(config).unwrap();

    let results = engine
        .on_market_snapshot(market_at(99, 100, 1_700_000_001))
        .unwrap();

    assert!(results.is_empty());
    assert_eq!(engine.position(&btc()).unwrap().quantity, cash(1));
}

#[test]
fn invalid_initial_inventory_is_rejected() {
    let config =
        SimulationConfig::new(Decimal::ZERO).with_initial_position(btc(), Decimal::ZERO, cash(100));

    let error = SimulationEngine::new(config).unwrap_err();

    assert!(matches!(
        error,
        SimulationError::InvalidConfig {
            field: "initial_position.quantity",
            ..
        }
    ));
}

#[test]
fn filled_order_cannot_be_cancelled() {
    let mut engine = SimulationEngine::new(SimulationConfig::new(cash(1_000))).unwrap();
    let request = ExecutionRequest::market(
        btc(),
        TradeAction::Buy,
        OrderSize::Quantity(cash(1)),
        timestamp(1_700_000_000),
    );
    let result = engine.execute(request, market(99, 100)).unwrap();

    let error = engine.cancel_order(result.order_id).unwrap_err();

    assert!(matches!(error, SimulationError::OrderAlreadyFinal(_)));
}
