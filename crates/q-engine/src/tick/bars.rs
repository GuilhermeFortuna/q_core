//! Tick → OHLCV bar aggregation and display-interval helpers.

use super::numpy_sum::numpy_sum_f64;
use super::simulate::TickError;

/// Resampled OHLCV bars with half-open tick index ranges `[tick_start, tick_end)`.
#[derive(Clone, Debug, Default)]
pub struct TickBars {
    /// Bucket open time: `bucket * bar_ms`.
    pub open_msc: Vec<i64>,
    pub open: Vec<f64>,
    pub high: Vec<f64>,
    pub low: Vec<f64>,
    pub close: Vec<f64>,
    /// `trunc` of numpy-order volume sum.
    pub volume: Vec<i64>,
    pub tick_start: Vec<i64>,
    pub tick_end: Vec<i64>,
}

/// Aggregate ticks into bars of width `bar_ms`.
pub fn tick_bars(
    time_msc: &[i64],
    bid: &[f64],
    ask: &[f64],
    last: &[f64],
    volume: &[f64],
    bar_ms: i64,
) -> Result<TickBars, TickError> {
    let n = time_msc.len();
    check_len("bid", n, bid.len())?;
    check_len("ask", n, ask.len())?;
    check_len("last", n, last.len())?;
    check_len("volume", n, volume.len())?;

    if n == 0 {
        return Ok(TickBars::default());
    }

    let use_last = last.iter().any(|&v| v > 0.0);
    let mut out = TickBars::default();

    let mut starts = vec![0usize];
    let mut prev_bucket = time_msc[0].div_euclid(bar_ms);
    for (i, &t) in time_msc.iter().enumerate().skip(1) {
        let bucket = t.div_euclid(bar_ms);
        if bucket != prev_bucket {
            starts.push(i);
            prev_bucket = bucket;
        }
    }
    let mut ends = starts[1..].to_vec();
    ends.push(n);

    for (bar_idx, (&start, &end)) in starts.iter().zip(ends.iter()).enumerate() {
        let bucket = time_msc[start].div_euclid(bar_ms);
        let open_msc = bucket * bar_ms;
        let (open, high, low, close) = ohlc_slice(bid, ask, last, start, end, use_last);
        let vol_sum = numpy_sum_f64(&volume[start..end]);
        if !vol_sum.is_finite() {
            return Err(TickError::VolumeNotFinite { bar: bar_idx });
        }

        out.open_msc.push(open_msc);
        out.open.push(open);
        out.high.push(high);
        out.low.push(low);
        out.close.push(close);
        out.volume.push(vol_sum.trunc() as i64);
        out.tick_start.push(start as i64);
        out.tick_end.push(end as i64);
    }

    Ok(out)
}

fn check_len(column: &'static str, expected: usize, actual: usize) -> Result<(), TickError> {
    if expected == actual {
        Ok(())
    } else {
        Err(TickError::LengthMismatch {
            column,
            expected,
            actual,
        })
    }
}

fn price_at(bid: &[f64], ask: &[f64], last: &[f64], i: usize, use_last: bool) -> f64 {
    if use_last {
        last[i]
    } else {
        (bid[i] + ask[i]) / 2.0
    }
}

/// High/low propagate NaN like `np.max` / `np.min` (NaN as soon as one is seen).
fn ohlc_slice(
    bid: &[f64],
    ask: &[f64],
    last: &[f64],
    start: usize,
    end: usize,
    use_last: bool,
) -> (f64, f64, f64, f64) {
    let open = price_at(bid, ask, last, start, use_last);
    let mut high = open;
    let mut low = open;
    let mut close = open;
    for i in (start + 1)..end {
        let p = price_at(bid, ask, last, i, use_last);
        close = p;
        if high.is_nan() || p.is_nan() {
            high = f64::NAN;
        } else if p > high {
            high = p;
        }
        if low.is_nan() || p.is_nan() {
            low = f64::NAN;
        } else if p < low {
            low = p;
        }
    }
    (open, high, low, close)
}

const MAX_DISPLAY_BARS: i64 = 50_000;
const D1_MS: i64 = 86_400_000;

/// Double `base_bar_ms` until the span fits under the display bar cap (port of `_resolve_bar_ms`).
pub fn resolve_bar_ms(base_bar_ms: i64, span_msc: i64) -> i64 {
    let mut bar_ms = base_bar_ms;
    while span_msc > 0 && span_msc.div_euclid(bar_ms) > MAX_DISPLAY_BARS {
        bar_ms *= 2;
        if bar_ms > D1_MS {
            break;
        }
    }
    bar_ms
}

/// Sample `series` at each bar's last tick (`tick_end[i] - 1`). `NaN` means missing.
pub fn sample_at_bar_ends(series: &[f64], tick_end: &[i64]) -> Vec<f64> {
    tick_end
        .iter()
        .map(|&end| series[(end - 1) as usize])
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{resolve_bar_ms, sample_at_bar_ends, tick_bars};
    use crate::tick::TickError;

    #[test]
    fn last_all_zero_uses_midpoint() {
        let time = [0_i64, 1, 2];
        let bid = [10.0, 20.0, 30.0];
        let ask = [12.0, 22.0, 32.0];
        let last = [0.0, 0.0, 0.0];
        let vol = [1.0, 1.0, 1.0];
        let bars = tick_bars(&time, &bid, &ask, &last, &vol, 60_000).unwrap();
        assert_eq!(bars.open.len(), 1);
        assert_eq!(bars.open[0].to_bits(), 11.0_f64.to_bits());
        assert_eq!(bars.high[0].to_bits(), 31.0_f64.to_bits());
        assert_eq!(bars.low[0].to_bits(), 11.0_f64.to_bits());
        assert_eq!(bars.close[0].to_bits(), 31.0_f64.to_bits());
    }

    #[test]
    fn one_positive_last_uses_last_everywhere() {
        let time = [0_i64, 1, 2];
        let bid = [10.0, 20.0, 30.0];
        let ask = [12.0, 22.0, 32.0];
        let last = [0.0, 0.01, 0.0];
        let vol = [1.0, 1.0, 1.0];
        let bars = tick_bars(&time, &bid, &ask, &last, &vol, 60_000).unwrap();
        assert_eq!(bars.open[0].to_bits(), 0.0_f64.to_bits());
        assert_eq!(bars.high[0].to_bits(), 0.01_f64.to_bits());
        assert_eq!(bars.low[0].to_bits(), 0.0_f64.to_bits());
        assert_eq!(bars.close[0].to_bits(), 0.0_f64.to_bits());
    }

    #[test]
    fn nan_price_makes_high_low_nan_but_not_open() {
        let time = [0_i64, 1];
        let bid = [10.0, 20.0];
        // last all non-positive → midpoint; second mid is NaN via NaN ask.
        let last = [0.0, 0.0];
        let vol = [1.0, 1.0];
        let ask_nan = [12.0, f64::NAN];
        let bars = tick_bars(&time, &bid, &ask_nan, &last, &vol, 60_000).unwrap();
        assert!(bars.open[0].is_finite(), "open={}", bars.open[0]);
        assert!(bars.high[0].is_nan(), "high={}", bars.high[0]);
        assert!(bars.low[0].is_nan(), "low={}", bars.low[0]);
    }

    #[test]
    fn volumes_half_plus_six_tenths_truncate_to_one() {
        let time = [0_i64, 1];
        let bid = [1.0, 1.0];
        let ask = [1.0, 1.0];
        let last = [1.0, 1.0];
        let vol = [0.5, 0.6];
        let bars = tick_bars(&time, &bid, &ask, &last, &vol, 60_000).unwrap();
        assert_eq!(bars.volume, vec![1]);
    }

    #[test]
    fn nan_volume_is_volume_not_finite() {
        let time = [0_i64];
        let bid = [1.0];
        let ask = [1.0];
        let last = [1.0];
        let vol = [f64::NAN];
        let err = tick_bars(&time, &bid, &ask, &last, &vol, 60_000).unwrap_err();
        assert_eq!(err, TickError::VolumeNotFinite { bar: 0 });
    }

    #[test]
    fn resolve_bar_ms_doubles_m1_for_forty_days() {
        assert_eq!(resolve_bar_ms(60_000, 3_456_000_000), 120_000);
    }

    #[test]
    fn resolve_bar_ms_doubles_h12_past_d1() {
        assert_eq!(resolve_bar_ms(43_200_000, 5_000_000_000_000), 172_800_000);
    }

    #[test]
    fn resolve_bar_ms_zero_span_keeps_base() {
        assert_eq!(resolve_bar_ms(60_000, 0), 60_000);
    }

    #[test]
    fn sample_at_bar_ends_returns_series_at_end_minus_one() {
        let series = [10.0, 20.0, 30.0, 40.0];
        let tick_end = [2_i64, 4];
        let sampled = sample_at_bar_ends(&series, &tick_end);
        assert_eq!(sampled[0].to_bits(), 20.0_f64.to_bits());
        assert_eq!(sampled[1].to_bits(), 40.0_f64.to_bits());
    }
}
