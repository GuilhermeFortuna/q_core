#![forbid(unsafe_code)]
//! Computational kernels for candle and tick simulations, fill models, exit rules, and position sizing.
//!
//! Implements deterministic trade evaluation and execution state machines consumed identically
//! by research backtesting and live evaluation paths.

pub mod candle;
pub mod exits;

pub use candle::{CandleConfig, CandleError, Costs, DayTradeWindow, Sizing};
pub use exits::{
    ExitBook, ExitDecision, ExitError, ExitInputs, ExitParams, ExitRuleId, ExitRuleSet,
    OpenPosition, ParamValue, PositionKey, PsarState, RuleState, Side,
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
