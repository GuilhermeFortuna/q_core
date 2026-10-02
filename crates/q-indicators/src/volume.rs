//! Deterministic tape-derived volume studies.
//!
//! Callers provide trades in source order, chart bar intervals, and session keys.
//! This module does not interpret exchange clocks, coverage, deduplication, or
//! volume units.

use std::collections::VecDeque;

/// Raw flag bit for an aggressor buy.
pub const BUY_FLAG: u32 = 1 << 5;
/// Raw flag bit for an aggressor sell.
pub const SELL_FLAG: u32 = 1 << 6;
/// Maximum number of trades retained by the bounded rate window.
pub const MAX_RATE_EVENTS: usize = 100_000;

/// Classification derived only from the supplied aggressor flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AggressorSide {
    Buy,
    Sell,
    Unknown,
}

impl AggressorSide {
    /// Classifies flags when exactly one recognized side bit is set.
    pub fn from_flags(raw_flags: u32) -> Self {
        match (raw_flags & BUY_FLAG != 0, raw_flags & SELL_FLAG != 0) {
            (true, false) => Self::Buy,
            (false, true) => Self::Sell,
            _ => Self::Unknown,
        }
    }
}

/// One already validated trade and its caller-owned chart/session context.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TradeInput {
    pub time_msc: i64,
    pub price: f64,
    pub volume: f64,
    pub raw_flags: u32,
    pub session_key: u64,
    pub bar_open_msc: i64,
    pub bar_close_msc: i64,
}

/// Configuration for tape rate and large-print observations.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VolumeConfig {
    pub window_ms: i64,
    pub large_print_threshold: f64,
}

impl Default for VolumeConfig {
    fn default() -> Self {
        Self {
            window_ms: 10_000,
            large_print_threshold: 100.0,
        }
    }
}

/// A trade that met the configured individual-print threshold.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LargePrint {
    pub side: AggressorSide,
    pub time_msc: i64,
    pub price: f64,
    pub volume: f64,
}

/// Current volume statistics for a chart bar and its session.
#[derive(Debug, Clone, PartialEq)]
pub struct VolumeOutput {
    pub bar_open_msc: Option<i64>,
    pub bar_close_msc: Option<i64>,
    pub buy_volume: f64,
    pub sell_volume: f64,
    pub unknown_volume: f64,
    pub total_volume: f64,
    pub delta: f64,
    pub cumulative_delta: f64,
    pub classified_share: Option<f64>,
    /// Trades per second, unavailable until the full window is observed.
    pub trade_rate: Option<f64>,
    pub rate_warmed_up: bool,
    /// Qualifying prints from the current chart bar.
    pub large_prints: Vec<LargePrint>,
}

impl Default for VolumeOutput {
    fn default() -> Self {
        Self {
            bar_open_msc: None,
            bar_close_msc: None,
            buy_volume: 0.0,
            sell_volume: 0.0,
            unknown_volume: 0.0,
            total_volume: 0.0,
            delta: 0.0,
            cumulative_delta: 0.0,
            classified_share: None,
            trade_rate: None,
            rate_warmed_up: false,
            large_prints: Vec::new(),
        }
    }
}

/// A rejected configuration, trade, checkpoint, or bounded-window update.
#[derive(Debug, Clone, PartialEq)]
pub enum VolumeError {
    InvalidConfig { field: &'static str },
    InvalidTrade { field: &'static str },
    InvalidBarInterval,
    DecreasingTime { previous: i64, next: i64 },
    CheckpointBeforeLastInput { checkpoint: i64, last_input: i64 },
    CapacityExceeded { capacity: usize },
    ArithmeticOverflow,
}

impl core::fmt::Display for VolumeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::InvalidConfig { field } => write!(f, "volume: invalid config field {field}"),
            Self::InvalidTrade { field } => write!(f, "volume: invalid trade field {field}"),
            Self::InvalidBarInterval => write!(f, "volume: bar interval must have positive width"),
            Self::DecreasingTime { previous, next } => {
                write!(f, "volume: time decreased from {previous} to {next}")
            }
            Self::CheckpointBeforeLastInput {
                checkpoint,
                last_input,
            } => write!(
                f,
                "volume: checkpoint {checkpoint} precedes last input {last_input}"
            ),
            Self::CapacityExceeded { capacity } => {
                write!(f, "volume: rate event capacity {capacity} exceeded")
            }
            Self::ArithmeticOverflow => write!(f, "volume: accumulator overflow"),
        }
    }
}

impl std::error::Error for VolumeError {}

#[derive(Debug, Clone, Copy)]
struct RateEvent {
    time_msc: i64,
}

/// Incremental deterministic state for bar volume, cumulative delta, and tape rate.
#[derive(Debug, Clone)]
pub struct VolumeState {
    config: VolumeConfig,
    session_key: Option<u64>,
    bar_open_msc: Option<i64>,
    bar_close_msc: Option<i64>,
    buy_volume: f64,
    sell_volume: f64,
    unknown_volume: f64,
    cumulative_delta: f64,
    large_prints: Vec<LargePrint>,
    rate_events: VecDeque<RateEvent>,
    coverage_start_msc: Option<i64>,
    last_time_msc: Option<i64>,
}

impl VolumeState {
    /// Creates an empty state after validating its configuration.
    pub fn new(config: VolumeConfig) -> Result<Self, VolumeError> {
        validate_config(config)?;
        Ok(Self {
            config,
            session_key: None,
            bar_open_msc: None,
            bar_close_msc: None,
            buy_volume: 0.0,
            sell_volume: 0.0,
            unknown_volume: 0.0,
            cumulative_delta: 0.0,
            large_prints: Vec::new(),
            rate_events: VecDeque::new(),
            coverage_start_msc: None,
            last_time_msc: None,
        })
    }

    /// Adds one trade. Rejected trades leave this state unchanged.
    pub fn push(&mut self, trade: TradeInput) -> Result<VolumeOutput, VolumeError> {
        self.push_inner(trade)?;
        Ok(self.current())
    }

    fn push_inner(&mut self, trade: TradeInput) -> Result<(), VolumeError> {
        validate_trade(trade)?;
        if let Some(previous) = self.last_time_msc {
            if trade.time_msc < previous {
                return Err(VolumeError::DecreasingTime {
                    previous,
                    next: trade.time_msc,
                });
            }
        }

        let session_changed = self.session_key.is_some_and(|key| key != trade.session_key);
        let bar_changed = self
            .bar_open_msc
            .is_some_and(|open| open != trade.bar_open_msc)
            || self
                .bar_close_msc
                .is_some_and(|close| close != trade.bar_close_msc);
        let left_edge = (trade.time_msc as i128) - (self.config.window_ms as i128);
        let expired = if session_changed {
            self.rate_events.len()
        } else {
            self.rate_events
                .iter()
                .take_while(|event| (event.time_msc as i128) <= left_edge)
                .count()
        };
        let surviving = self.rate_events.len() - expired;
        if surviving >= MAX_RATE_EVENTS {
            return Err(VolumeError::CapacityExceeded {
                capacity: MAX_RATE_EVENTS,
            });
        }
        let side = AggressorSide::from_flags(trade.raw_flags);
        let (buy, sell, unknown) = match side {
            AggressorSide::Buy => (trade.volume, 0.0, 0.0),
            AggressorSide::Sell => (0.0, trade.volume, 0.0),
            AggressorSide::Unknown => (0.0, 0.0, trade.volume),
        };
        let clear_bar = session_changed || bar_changed;
        let prior_buy = if clear_bar { 0.0 } else { self.buy_volume };
        let prior_sell = if clear_bar { 0.0 } else { self.sell_volume };
        let prior_unknown = if clear_bar { 0.0 } else { self.unknown_volume };
        let prior_cvd = if session_changed {
            0.0
        } else {
            self.cumulative_delta
        };
        let next_buy = finite_sum(prior_buy, buy)?;
        let next_sell = finite_sum(prior_sell, sell)?;
        let next_unknown = finite_sum(prior_unknown, unknown)?;
        finite_sum(next_buy, -next_sell)?;
        let next_cvd = finite_sum(prior_cvd, finite_sum(buy, -sell)?)?;
        finite_sum(finite_sum(next_buy, next_sell)?, next_unknown)?;

        if session_changed {
            self.rate_events.clear();
            self.coverage_start_msc = Some(trade.time_msc);
        } else if self.coverage_start_msc.is_none() {
            self.coverage_start_msc = Some(trade.time_msc);
        }
        if clear_bar {
            self.large_prints.clear();
        }
        for _ in 0..expired {
            self.rate_events.pop_front();
        }
        self.buy_volume = next_buy;
        self.sell_volume = next_sell;
        self.unknown_volume = next_unknown;
        self.cumulative_delta = next_cvd;
        self.bar_open_msc = Some(trade.bar_open_msc);
        self.bar_close_msc = Some(trade.bar_close_msc);
        if trade.volume >= self.config.large_print_threshold {
            self.large_prints.push(LargePrint {
                side,
                time_msc: trade.time_msc,
                price: trade.price,
                volume: trade.volume,
            });
        }
        self.rate_events.push_back(RateEvent {
            time_msc: trade.time_msc,
        });
        self.session_key = Some(trade.session_key);
        self.last_time_msc = Some(trade.time_msc);
        Ok(())
    }

    /// Advances the explicit observation time, aging the rate window without a trade.
    pub fn advance(&mut self, now_msc: i64) -> Result<VolumeOutput, VolumeError> {
        if let Some(previous) = self.last_time_msc {
            if now_msc < previous {
                return Err(VolumeError::DecreasingTime {
                    previous,
                    next: now_msc,
                });
            }
        }
        self.last_time_msc = Some(now_msc);
        let left_edge = (now_msc as i128) - (self.config.window_ms as i128);
        while self
            .rate_events
            .front()
            .is_some_and(|event| (event.time_msc as i128) <= left_edge)
        {
            self.rate_events.pop_front();
        }
        if self.coverage_start_msc.is_none() {
            self.coverage_start_msc = Some(now_msc);
        }
        Ok(self.current())
    }

    /// Returns the current bar and session state without advancing time.
    pub fn current(&self) -> VolumeOutput {
        let total_volume = self.buy_volume + self.sell_volume + self.unknown_volume;
        let delta = self.buy_volume - self.sell_volume;
        let classified_total = self.buy_volume + self.sell_volume;
        let rate_warmed_up = self.coverage_start_msc.is_some_and(|start| {
            (self.last_time_msc.unwrap_or(start) as i128) - (start as i128)
                >= self.config.window_ms as i128
        });
        let trade_rate = if rate_warmed_up {
            Some(self.rate_events.len() as f64 / (self.config.window_ms as f64 / 1_000.0))
        } else {
            None
        };
        VolumeOutput {
            bar_open_msc: self.bar_open_msc,
            bar_close_msc: self.bar_close_msc,
            buy_volume: self.buy_volume,
            sell_volume: self.sell_volume,
            unknown_volume: self.unknown_volume,
            total_volume,
            delta,
            cumulative_delta: self.cumulative_delta,
            classified_share: if total_volume > 0.0 {
                Some(classified_total / total_volume)
            } else {
                None
            },
            trade_rate,
            rate_warmed_up,
            large_prints: self.large_prints.clone(),
        }
    }

    /// Clears all observations while retaining the current configuration.
    pub fn reset(&mut self) {
        if let Ok(empty) = Self::new(self.config) {
            *self = empty;
        }
    }
}

/// Replays trades once and samples state at ordered checkpoints.
///
/// The returned vector contains one output per input trade, followed by one output
/// per checkpoint. Checkpoints must be nondecreasing and no earlier than the final
/// trade timestamp.
pub fn batch_volume(
    config: VolumeConfig,
    trades: &[TradeInput],
    checkpoints: &[i64],
) -> Result<Vec<VolumeOutput>, VolumeError> {
    let mut state = VolumeState::new(config)?;
    let mut outputs = Vec::with_capacity(trades.len() + checkpoints.len());
    for trade in trades {
        outputs.push(state.push(*trade)?);
    }
    let last_input = trades.last().map(|trade| trade.time_msc);
    let mut previous_checkpoint = last_input;
    for &checkpoint in checkpoints {
        if let Some(last) = last_input {
            if checkpoint < last {
                return Err(VolumeError::CheckpointBeforeLastInput {
                    checkpoint,
                    last_input: last,
                });
            }
        }
        if let Some(previous) = previous_checkpoint {
            if checkpoint < previous {
                return Err(VolumeError::DecreasingTime {
                    previous,
                    next: checkpoint,
                });
            }
        }
        outputs.push(state.advance(checkpoint)?);
        previous_checkpoint = Some(checkpoint);
    }
    Ok(outputs)
}

fn validate_config(config: VolumeConfig) -> Result<(), VolumeError> {
    if !(1_000..=300_000).contains(&config.window_ms) {
        return Err(VolumeError::InvalidConfig { field: "window_ms" });
    }
    if !config.large_print_threshold.is_finite() || config.large_print_threshold <= 0.0 {
        return Err(VolumeError::InvalidConfig {
            field: "large_print_threshold",
        });
    }
    Ok(())
}

fn validate_trade(trade: TradeInput) -> Result<(), VolumeError> {
    if !trade.price.is_finite() || trade.price <= 0.0 {
        return Err(VolumeError::InvalidTrade { field: "price" });
    }
    if !trade.volume.is_finite() || trade.volume <= 0.0 {
        return Err(VolumeError::InvalidTrade { field: "volume" });
    }
    if trade.bar_open_msc >= trade.bar_close_msc
        || trade.time_msc < trade.bar_open_msc
        || trade.time_msc >= trade.bar_close_msc
    {
        return Err(VolumeError::InvalidBarInterval);
    }
    Ok(())
}

fn finite_sum(left: f64, right: f64) -> Result<f64, VolumeError> {
    let sum = left + right;
    if sum.is_finite() {
        Ok(sum)
    } else {
        Err(VolumeError::ArithmeticOverflow)
    }
}
