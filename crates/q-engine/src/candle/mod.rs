mod config;
mod decision;
mod inputs;
mod protective;
mod run;
mod session_ticks;
mod sizing;

pub(crate) use config::side_cost;
pub use config::{CandleConfig, CandleError, Costs, DayTradeWindow};
pub use decision::{Decision, DecisionStep, QueuedEntry, QueuedExit, TradeView};
pub use inputs::{BarSignals, CandleInputs, EntryCandidate, OcoOrder, SignalColumns};
pub use protective::{
    IntrabarPrices, IntrabarSource, ProtectiveColumns, RejectedEntries, RestingKind, RestingOrder,
};
pub use run::{
    run_candle, run_candle_with_callback, run_candle_with_callbacks, CandleRun, DecisionTrace,
    EntryFilledFn, ExitReason, PositionSnapshot, RuntimeCallbacks, StrategyFn, TradeLedger,
};
pub use session_ticks::{
    SessionLoader, SessionTickBars, SessionTickError, SessionTicks, TradePrices,
};
pub use sizing::Sizing;
