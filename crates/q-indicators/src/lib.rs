#![forbid(unsafe_code)]
//! Pure, deterministic technical indicator mathematics and streaming indicator state.
//!
//! Evaluates technical indicators over numerical series with no I/O, no system clock
//! or environment access, and strictly reproducible results across runs.

mod elementwise;
mod error;
mod ieee;
mod indicators;
mod moving_averages;
mod streaming;
mod transforms;
pub mod volume;
mod window;

pub use error::IndicatorError;
pub use indicators::*;
pub use moving_averages::*;
pub use streaming::*;
pub use transforms::*;
pub use volume::{
    batch_volume, AggressorSide, LargePrint, TradeInput, VolumeConfig, VolumeError, VolumeOutput,
    VolumeState, BUY_FLAG, MAX_RATE_EVENTS, SELL_FLAG,
};

/// Build identity for this crate (kernels and `IndicatorError` are also public).
pub const CRATE_NAME: &str = "q-indicators";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_crate_name() {
        assert_eq!(CRATE_NAME, "q-indicators");
    }
}
