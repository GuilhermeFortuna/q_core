//! Columnar inputs for the candle kernel: Q-024's decision columns and the bar series.
/// Strategy decisions for one closed bar, before engine gating and exit rules.
#[derive(Clone, Copy, Debug, Default)]
pub struct BarSignals {
    pub entry: i8,
    pub exit_long: bool,
    pub exit_short: bool,
    pub strength: f64,
    /// Stop and target for an entry queued on this bar, `NaN` where unset. `None` keeps the
    /// static protective columns.
    pub levels: Option<(f64, f64)>,
    /// Fill price for an entry queued on this bar, `NaN` for none. `None` keeps the static
    /// `entry_price` column.
    pub entry_price: Option<f64>,
}

impl BarSignals {
    pub fn at(signals: &SignalColumns<'_>, bar: usize) -> Self {
        Self {
            entry: signals.entry[bar],
            exit_long: signals.exit_long[bar],
            exit_short: signals.exit_short[bar],
            strength: signals.strength[bar],
            levels: None,
            entry_price: None,
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

use super::protective::{IntrabarSource, ProtectiveColumns};

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
