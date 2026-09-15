//! Tick simulation and bar aggregation kernels.

mod numpy_sum;
mod simulate;

pub use simulate::{
    simulate_ticks, TickError, TickExitReason, TickInputs, TickLedger, TickRun, TickSizing,
};
