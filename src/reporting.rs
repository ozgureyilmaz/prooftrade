//! Deterministic reporting metrics for simulation and operator read models.

use rust_decimal::Decimal;
use serde::Serialize;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SimulationReport {
    pub starting_equity: Decimal,
    pub ending_equity: Decimal,
    pub pnl: Decimal,
    pub max_drawdown: Decimal,
    pub trade_count: usize,
    pub wins: usize,
    pub losses: usize,
    pub fees: Decimal,
    pub slippage: Decimal,
    pub hold_count: usize,
    pub rejected_count: usize,
    pub protective_exit_count: usize,
}

pub struct ReportBuilder {
    starting_equity: Decimal,
    peak_equity: Decimal,
    max_drawdown: Decimal,
    realized_pnl: Decimal,
    trade_count: usize,
    wins: usize,
    losses: usize,
    fees: Decimal,
    slippage: Decimal,
    hold_count: usize,
    rejected_count: usize,
    protective_exit_count: usize,
}

impl ReportBuilder {
    pub fn new(starting_equity: Decimal) -> Self {
        Self {
            starting_equity,
            peak_equity: starting_equity,
            max_drawdown: Decimal::ZERO,
            realized_pnl: Decimal::ZERO,
            trade_count: 0,
            wins: 0,
            losses: 0,
            fees: Decimal::ZERO,
            slippage: Decimal::ZERO,
            hold_count: 0,
            rejected_count: 0,
            protective_exit_count: 0,
        }
    }

    pub fn record_equity(&mut self, equity: Decimal) {
        if equity > self.peak_equity {
            self.peak_equity = equity;
        }
        let drawdown = self.peak_equity - equity;
        if drawdown > self.max_drawdown {
            self.max_drawdown = drawdown;
        }
    }

    pub fn record_trade(&mut self, realized_pnl: Decimal, fee: Decimal) {
        self.trade_count += 1;
        if realized_pnl > Decimal::ZERO {
            self.wins += 1;
        } else if realized_pnl < Decimal::ZERO {
            self.losses += 1;
        }
        self.realized_pnl += realized_pnl;
        self.fees += fee;
    }

    pub fn record_slippage(&mut self, amount: Decimal) {
        self.slippage += amount;
    }

    pub fn record_hold(&mut self) {
        self.hold_count += 1;
    }

    pub fn record_rejection(&mut self) {
        self.rejected_count += 1;
    }

    pub fn record_protective_exit(&mut self) {
        self.protective_exit_count += 1;
    }

    pub fn finish(self, ending_equity: Decimal) -> SimulationReport {
        SimulationReport {
            starting_equity: self.starting_equity,
            ending_equity,
            pnl: ending_equity - self.starting_equity,
            max_drawdown: self.max_drawdown,
            trade_count: self.trade_count,
            wins: self.wins,
            losses: self.losses,
            fees: self.fees,
            slippage: self.slippage,
            hold_count: self.hold_count,
            rejected_count: self.rejected_count,
            protective_exit_count: self.protective_exit_count,
        }
    }
}
