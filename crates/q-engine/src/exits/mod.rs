//! Exit-rule state machines: deterministic per-position update and decide over bar columns.
//!
//! Ports the eleven backend exit rules as ordered state machines over columnar bar
//! inputs. A rule set is built from the same parameter names and defaults as
//! `q_backend.backtesting.exit_strategy.ExitStrategy`, and [`ExitBook::evaluate`]
//! matches `check_exits`: prune stale position state, then for each open position
//! update and decide each enabled rule in registry order until one fires.
//!
//! # Rules and enabling parameters
//!
//! | Id | Parameter(s) | Default enable |
//! |---|---|---|
//! | `fixed_sl` | `stop_loss_pct` | 0.0 (off) |
//! | `atr_sl` | `stop_loss_atr`, `atr_period` (14) | 0.0 (off) |
//! | `fixed_tp` | `take_profit_pct` | 0.0 (off) |
//! | `atr_tp` | `take_profit_atr`, `atr_period` | 0.0 (off) |
//! | `trailing` | `trailing_stop_pct` | 0.0 (off) |
//! | `chandelier` | `chandelier_atr_mult`, `atr_period` | 0.0 (off) |
//! | `breakeven` | `breakeven_trigger_pct`, `breakeven_offset_pct` | 0.0 (off) |
//! | `psar` | `psar_af_start`, `psar_af_step` (0.02), `psar_af_max` (0.2) | 0.0 (off) |
//! | `profit_target_ratchet` | `target_ratchet_atr`, `atr_period` | 0.0 (off) |
//! | `time_stop` | `max_bars_in_trade` | 0 (off) |
//! | `donchian_stop` | `donchian_exit_period` | 0 (off) |
//!
//! A rule is enabled exactly when its enabling parameter is greater than zero.
//!
//! # Reproduced quirks (goldens hold them still)
//!
//! - When a rule fires, later rules are neither updated nor asked on that bar.
//! - The time stop increments on the fill bar (first evaluation counts as bar 1).
//! - The profit-target ratchet does not arm on a bar where ATR is missing.
//! - High/low extrema use Python `max`/`min` operand rules for NaN (left wins
//!   unless the right strictly compares).

pub mod book;
pub(crate) mod logic;
pub mod params;
pub(crate) mod pyops;
pub mod rules;

pub use book::{ExitBook, ExitDecision, ExitInputs, OpenPosition, PositionKey};
pub use params::{ExitError, ExitParams, ParamValue};
pub use rules::{ExitRuleId, ExitRuleSet, PsarState, RuleState, Side};
