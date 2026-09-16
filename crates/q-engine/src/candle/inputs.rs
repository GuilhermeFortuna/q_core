//! Columnar inputs for the candle kernel: Q-024's decision columns and the bar series.

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
}
