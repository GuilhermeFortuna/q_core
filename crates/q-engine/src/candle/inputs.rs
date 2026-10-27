//! Columnar inputs for the candle kernel: Q-024's decision columns and the bar series.
/// A priced entry offered on its deciding bar. Among a bar's candidates the first the tape touches
/// fills and the others are cancelled.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EntryCandidate {
    pub side: Side,
    pub price: f64,
    /// Stop and target, `NaN` where unset. `None` takes the bar's `levels`.
    pub levels: Option<(f64, f64)>,
}

/// Strategy decisions for one closed bar, before engine gating and exit rules.
#[derive(Clone, Debug, Default)]
pub struct BarSignals {
    pub entry: i8,
    pub exit_long: bool,
    pub exit_short: bool,
    pub strength: f64,
    /// Stop and target for an entry queued on this bar, `NaN` where unset. `None` keeps the
    /// static protective columns.
    pub levels: Option<(f64, f64)>,
    /// Priced entries for this bar, filled on it: the first one the tape touches wins. `Some` of an
    /// empty list means none. `None` keeps the static `entry_price` column.
    pub entry_candidates: Option<Vec<EntryCandidate>>,
    /// Places a resting order instead of a market or priced entry. Requires `entry != 0`.
    pub resting: Option<RestingOrder>,
    /// Removes the working resting order without placing another.
    pub cancel_entry: bool,
    /// Replaces the stop of every open trade the callback saw; `None` keeps it.
    pub amend_stop: Option<f64>,
    /// Replaces the target of every open trade the callback saw; `None` keeps it.
    pub amend_target: Option<f64>,
    /// Prices at which a same-decision signal close may fill intrabar: the first one the tape
    /// reaches wins. Empty means the close fills at the next open.
    pub exit_prices: Vec<f64>,
    /// With `exit_prices`, the close also queues for the next open when none of them is reached.
    pub close_next_open: bool,
}

impl BarSignals {
    pub fn at(signals: &SignalColumns<'_>, bar: usize) -> Self {
        Self {
            entry: signals.entry[bar],
            exit_long: signals.exit_long[bar],
            exit_short: signals.exit_short[bar],
            strength: signals.strength[bar],
            levels: None,
            entry_candidates: None,
            resting: None,
            cancel_entry: false,
            amend_stop: None,
            amend_target: None,
            exit_prices: Vec::new(),
            close_next_open: false,
        }
    }
}

/// Q-024's decision columns for one series.
pub struct SignalColumns<'a> {
    /// `+1` BUY, `-1` SELL, `0` none.
    pub entry: &'a [i8],
    pub exit_long: &'a [bool],
    pub exit_short: &'a [bool],
    /// In `[0, 1]` wherever `entry != 0`.
    pub strength: &'a [f64],
    /// Required iff a holding period is declared.
    pub bar_index: Option<&'a [i64]>,
}

use super::protective::{IntrabarSource, ProtectiveColumns, RestingOrder};
use crate::exits::Side;

/// Every column one run reads.
pub struct CandleInputs<'a> {
    /// Wall-clock microseconds, any order, repeats allowed.
    pub time_us: &'a [i64],
    pub open: Option<&'a [f64]>,
    pub high: Option<&'a [f64]>,
    pub low: Option<&'a [f64]>,
    pub close: Option<&'a [f64]>,
    pub signals: SignalColumns<'a>,
    pub volatility: Option<&'a [f64]>,
    /// `false` = before trade start: the bar is skipped entirely.
    pub tradable: Option<&'a [bool]>,
    /// Exit-rule indicator columns by name.
    pub columns: &'a dyn Fn(&str) -> Option<&'a [f64]>,
    /// Stop and target levels for entries; requires `intrabar`, `open`, `high` and `low`.
    pub protective: Option<ProtectiveColumns<'a>>,
    /// Trade prices of a bar, asked for only when the bar's range reaches an open level.
    pub intrabar: Option<&'a dyn IntrabarSource>,
}
