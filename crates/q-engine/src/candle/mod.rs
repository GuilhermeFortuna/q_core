mod config;
mod decision;
mod inputs;
mod run;
mod sizing;

pub(crate) use config::side_cost;
pub use config::{CandleConfig, CandleError, Costs, DayTradeWindow};
pub use decision::{Decision, DecisionStep, QueuedEntry, QueuedExit, TradeView};
pub use inputs::{CandleInputs, SignalColumns};
pub use run::{run_candle, CandleRun, DecisionTrace, ExitReason, TradeLedger};
pub use sizing::Sizing;
