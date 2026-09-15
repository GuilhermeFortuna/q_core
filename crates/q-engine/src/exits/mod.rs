//! Exit-rule state machines: deterministic per-position update and decide over bar columns.

pub mod params;
pub(crate) mod pyops;

pub use params::{ExitError, ExitParams, ParamValue};
