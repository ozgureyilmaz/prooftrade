use prooftrade::reporting::{ReportBuilder, SimulationReport};
use rust_decimal::Decimal;

#[test]
fn report_calculates_drawdown_and_keeps_operational_counts() {
    let mut builder = ReportBuilder::new(Decimal::from(1_000));
    builder.record_equity(Decimal::from(1_100));
    builder.record_equity(Decimal::from(990));
    builder.record_trade(Decimal::from(20), Decimal::from(2));
    builder.record_trade(Decimal::from(-10), Decimal::from(1));
    builder.record_hold();
    builder.record_rejection();
    builder.record_protective_exit();
    let report = builder.finish(Decimal::from(990));

    assert_eq!(report.starting_equity, Decimal::from(1_000));
    assert_eq!(report.ending_equity, Decimal::from(990));
    assert_eq!(report.trade_count, 2);
    assert_eq!(report.wins, 1);
    assert_eq!(report.losses, 1);
    assert_eq!(report.fees, Decimal::from(3));
    assert_eq!(report.hold_count, 1);
    assert_eq!(report.rejected_count, 1);
    assert_eq!(report.protective_exit_count, 1);
    assert_eq!(report.max_drawdown, Decimal::from(110));
}

#[test]
fn empty_report_has_zero_drawdown() {
    let report = ReportBuilder::new(Decimal::from(30)).finish(Decimal::from(30));
    assert_eq!(
        report,
        SimulationReport {
            starting_equity: Decimal::from(30),
            ending_equity: Decimal::from(30),
            pnl: Decimal::ZERO,
            max_drawdown: Decimal::ZERO,
            trade_count: 0,
            wins: 0,
            losses: 0,
            fees: Decimal::ZERO,
            slippage: Decimal::ZERO,
            hold_count: 0,
            rejected_count: 0,
            protective_exit_count: 0,
        }
    );
}
