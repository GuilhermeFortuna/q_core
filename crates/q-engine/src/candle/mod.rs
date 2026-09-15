mod config;
mod sizing;

#[expect(
    unused_imports,
    reason = "used by the candle run loop added in the next implementation step"
)]
pub(crate) use config::side_cost;
pub use config::{CandleConfig, CandleError, Costs, DayTradeWindow};
pub use sizing::Sizing;
