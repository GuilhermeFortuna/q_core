//! Exit-rule state machines: deterministic per-position update and decide over bar columns.

pub mod book;
pub(crate) mod logic;
pub mod params;
pub(crate) mod pyops;
pub mod rules;

pub use book::{ExitBook, ExitDecision, ExitInputs, OpenPosition, PositionKey};
pub use params::{ExitError, ExitParams, ParamValue};
pub use rules::{ExitRuleId, ExitRuleSet, PsarState, RuleState, Side};
