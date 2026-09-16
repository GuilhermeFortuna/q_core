//! Tick simulation and bar aggregation kernels.

mod bars;
mod days;
mod numpy_sum;
mod simulate;

pub use bars::{resolve_bar_ms, sample_at_bar_ends, tick_bars, TickBars};
pub use days::tick_day_bounds;
pub use simulate::{
    simulate_ticks, TickError, TickExitReason, TickInputs, TickLedger, TickRun, TickSizing,
};
