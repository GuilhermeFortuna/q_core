#![forbid(unsafe_code)]
//! Computational kernels for candle and tick simulations, fill models, exit rules, and position sizing.
//!
//! Implements deterministic trade evaluation and execution state machines consumed identically
//! by research backtesting and live evaluation paths.

pub mod candle;
pub mod exits;
pub mod tick;

pub use candle::{
    run_candle, CandleConfig, CandleError, CandleInputs, CandleRun, Costs, DayTradeWindow,
    Decision, DecisionStep, DecisionTrace, ExitReason, QueuedEntry, QueuedExit, SignalColumns,
    Sizing, TradeLedger, TradeView,
};
pub use exits::{
    ExitBook, ExitDecision, ExitError, ExitInputs, ExitParams, ExitRuleId, ExitRuleSet,
    OpenPosition, ParamValue, PositionKey, PsarState, RuleState, Side,
};
pub use tick::{
    resolve_bar_ms, sample_at_bar_ends, simulate_ticks, tick_bars, tick_day_bounds, TickBars,
    TickError, TickExitReason, TickInputs, TickLedger, TickRun, TickSizing,
};

/// The crate's build identity.
pub const CRATE_NAME: &str = "q-engine";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_crate_name() {
        assert_eq!(CRATE_NAME, "q-engine");
    }
}
