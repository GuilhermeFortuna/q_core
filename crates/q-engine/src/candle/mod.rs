mod config;
mod decision;
mod inputs;
mod sizing;

#[expect(
    unused_imports,
    reason = "used by the candle run loop added in the next implementation step"
)]
pub(crate) use config::side_cost;
pub use config::{CandleConfig, CandleError, Costs, DayTradeWindow};
pub use decision::{Decision, DecisionStep, QueuedEntry, QueuedExit, TradeView};
pub use inputs::{CandleInputs, SignalColumns};
pub use sizing::Sizing;
