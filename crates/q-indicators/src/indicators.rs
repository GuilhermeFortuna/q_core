//! Public technical indicator kernels.

use crate::elementwise::{clip_lower, diff, shift};
use crate::error::IndicatorError;
use crate::ieee::{com_from_alpha_period, com_from_span};
use crate::window::{ewm_mean, rolling_max, rolling_mean, rolling_min, rolling_std, rolling_var};

fn nan_skipping_max3(a: f64, b: f64, c: f64) -> f64 {
    let mut out = f64::NAN;
    for x in [a, b, c] {
        if x.is_nan() {
            continue;
        }
        if out.is_nan() || x > out {
            out = x;
        }
    }
    out
}

/// Rolling annualized close-to-close realized volatility.
pub fn realized_vol(
    close: &[f64],
    window: i64,
    periods_per_year: i64,
) -> Result<Vec<f64>, IndicatorError> {
    if window < 0 {
        return Err(IndicatorError::InvalidParameter {
            function: "realized_vol",
            parameter: "window",
            value: window,
            requirement: ">= 0",
        });
    }
    let prev = shift(close, 1);
    let mut log_ret = vec![f64::NAN; close.len()];
    for i in 0..close.len() {
        log_ret[i] = (close[i] / prev[i]).ln();
    }
    let std = rolling_std(&log_ret, window as usize);
    let scale = (periods_per_year as f64).sqrt();
    Ok(std.into_iter().map(|s| s * scale).collect())
}

/// Rolling annualized Yang–Zhang volatility.
pub fn yang_zhang(
    open: &[f64],
    high: &[f64],
    low: &[f64],
    close: &[f64],
    window: i64,
    periods_per_year: i64,
) -> Result<Vec<f64>, IndicatorError> {
    if open.len() != close.len() {
        return Err(IndicatorError::LengthMismatch {
            function: "yang_zhang",
            parameter: "open",
            expected: close.len(),
            actual: open.len(),
        });
    }
    if high.len() != close.len() {
        return Err(IndicatorError::LengthMismatch {
            function: "yang_zhang",
            parameter: "high",
            expected: close.len(),
            actual: high.len(),
        });
    }
    if low.len() != close.len() {
        return Err(IndicatorError::LengthMismatch {
            function: "yang_zhang",
            parameter: "low",
            expected: close.len(),
            actual: low.len(),
        });
    }
    if window < 0 || window == 1 {
        return Err(IndicatorError::InvalidParameter {
            function: "yang_zhang",
            parameter: "window",
            value: window,
            requirement: ">= 0 and != 1",
        });
    }
    let n = close.len();
    if window == 0 {
        return Ok(vec![f64::NAN; n]);
    }
    let prev_close = shift(close, 1);
    let mut overnight = vec![f64::NAN; n];
    let mut open_close = vec![f64::NAN; n];
    let mut rs = vec![f64::NAN; n];
    for i in 0..n {
        overnight[i] = (open[i] / prev_close[i]).ln();
        open_close[i] = (close[i] / open[i]).ln();
        let u = (high[i] / open[i]).ln();
        let d = (low[i] / open[i]).ln();
        let c = (close[i] / open[i]).ln();
        #[expect(
            clippy::suboptimal_flops,
            reason = "pandas evaluates a*b+c unfused; mul_add changes the result bits"
        )]
        {
            rs[i] = u * (u - c) + d * (d - c);
        }
    }
    let w = window as usize;
    let var_on = rolling_var(&overnight, w);
    let var_oc = rolling_var(&open_close, w);
    let mean_rs = rolling_mean(&rs, w);
    let k = 0.34 / (1.34 + (window + 1) as f64 / (window - 1) as f64);
    let scale = periods_per_year as f64;
    let mut out = vec![f64::NAN; n];
    for i in 0..n {
        #[expect(
            clippy::suboptimal_flops,
            reason = "pandas evaluates a*b+c unfused; mul_add changes the result bits"
        )]
        {
            let yz_var = (var_on[i] + k * var_oc[i]) + (1.0 - k) * mean_rs[i];
            out[i] = (yz_var * scale).sqrt();
        }
    }
    Ok(out)
}

/// Relative Strength Index (Wilder / pandas ewm alpha).
pub fn rsi(close: &[f64], period: i64) -> Result<Vec<f64>, IndicatorError> {
    if period < 1 {
        return Err(IndicatorError::InvalidParameter {
            function: "rsi",
            parameter: "period",
            value: period,
            requirement: ">= 1",
        });
    }
    let delta = diff(close);
    let gain = clip_lower(&delta, 0.0);
    let neg_delta: Vec<f64> = delta.iter().map(|&x| -x).collect();
    let loss = clip_lower(&neg_delta, 0.0);
    let com = com_from_alpha_period(period);
    let min_periods = period as usize;
    let avg_gain = ewm_mean(&gain, com, min_periods);
    let avg_loss = ewm_mean(&loss, com, min_periods);
    let mut out = vec![f64::NAN; close.len()];
    for i in 0..close.len() {
        let rs = avg_gain[i] / avg_loss[i];
        out[i] = 100.0 - (100.0 / (1.0 + rs));
    }
    Ok(out)
}

/// Bollinger band triple (upper, middle, lower).
pub struct BollingerBands {
    pub upper: Vec<f64>,
    pub middle: Vec<f64>,
    pub lower: Vec<f64>,
}

/// Bollinger bands from rolling mean and std.
pub fn bollinger_bands(
    close: &[f64],
    period: i64,
    num_std: f64,
) -> Result<BollingerBands, IndicatorError> {
    if period < 0 {
        return Err(IndicatorError::InvalidParameter {
            function: "bollinger_bands",
            parameter: "period",
            value: period,
            requirement: ">= 0",
        });
    }
    let w = period as usize;
    let middle = rolling_mean(close, w);
    let std = rolling_std(close, w);
    let mut upper = vec![f64::NAN; close.len()];
    let mut lower = vec![f64::NAN; close.len()];
    for i in 0..close.len() {
        #[expect(
            clippy::suboptimal_flops,
            reason = "pandas evaluates a*b+c unfused; mul_add changes the result bits"
        )]
        {
            upper[i] = middle[i] + num_std * std[i];
            lower[i] = middle[i] - num_std * std[i];
        }
    }
    Ok(BollingerBands {
        upper,
        middle,
        lower,
    })
}

/// MACD line, signal, and histogram.
pub struct Macd {
    pub line: Vec<f64>,
    pub signal: Vec<f64>,
    pub histogram: Vec<f64>,
}

/// MACD from three EMA spans.
pub fn macd(
    close: &[f64],
    fast_period: i64,
    slow_period: i64,
    signal_period: i64,
) -> Result<Macd, IndicatorError> {
    for (parameter, value) in [
        ("fast_period", fast_period),
        ("slow_period", slow_period),
        ("signal_period", signal_period),
    ] {
        if value < 1 {
            return Err(IndicatorError::InvalidParameter {
                function: "macd",
                parameter,
                value,
                requirement: ">= 1",
            });
        }
    }
    let fast = ewm_mean(close, com_from_span(fast_period), 0);
    let slow = ewm_mean(close, com_from_span(slow_period), 0);
    let mut line = vec![f64::NAN; close.len()];
    for i in 0..close.len() {
        line[i] = fast[i] - slow[i];
    }
    let signal = ewm_mean(&line, com_from_span(signal_period), 0);
    let mut histogram = vec![f64::NAN; close.len()];
    for i in 0..close.len() {
        histogram[i] = line[i] - signal[i];
    }
    Ok(Macd {
        line,
        signal,
        histogram,
    })
}

/// Donchian channel upper/lower (each length matches its input series).
pub struct DonchianChannels {
    pub upper: Vec<f64>,
    pub lower: Vec<f64>,
}

/// Donchian channels: shifted rolling max/min.
pub fn donchian_channels(
    high: &[f64],
    low: &[f64],
    period: i64,
) -> Result<DonchianChannels, IndicatorError> {
    if period < 0 {
        return Err(IndicatorError::InvalidParameter {
            function: "donchian_channels",
            parameter: "period",
            value: period,
            requirement: ">= 0",
        });
    }
    let p = period as usize;
    let upper = shift(&rolling_max(high, p), 1);
    let lower = shift(&rolling_min(low, p), 1);
    Ok(DonchianChannels { upper, lower })
}

/// Average True Range (Wilder / pandas ewm alpha).
pub fn atr(
    high: &[f64],
    low: &[f64],
    close: &[f64],
    period: i64,
) -> Result<Vec<f64>, IndicatorError> {
    if period < 1 {
        return Err(IndicatorError::InvalidParameter {
            function: "atr",
            parameter: "period",
            value: period,
            requirement: ">= 1",
        });
    }
    if high.len() != close.len() {
        return Err(IndicatorError::LengthMismatch {
            function: "atr",
            parameter: "high",
            expected: close.len(),
            actual: high.len(),
        });
    }
    if low.len() != close.len() {
        return Err(IndicatorError::LengthMismatch {
            function: "atr",
            parameter: "low",
            expected: close.len(),
            actual: low.len(),
        });
    }
    let prev_close = shift(close, 1);
    let mut tr = vec![f64::NAN; close.len()];
    for i in 0..close.len() {
        let tr1 = high[i] - low[i];
        let tr2 = (high[i] - prev_close[i]).abs();
        let tr3 = (low[i] - prev_close[i]).abs();
        tr[i] = nan_skipping_max3(tr1, tr2, tr3);
    }
    Ok(ewm_mean(
        &tr,
        com_from_alpha_period(period),
        period as usize,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_bits_eq(actual: &[f64], expected: &[f64]) {
        assert_eq!(actual.len(), expected.len(), "length mismatch");
        for (i, (&a, &e)) in actual.iter().zip(expected.iter()).enumerate() {
            if e.is_nan() {
                assert!(a.is_nan(), "index {i}: expected NaN, got {a}");
            } else {
                assert_eq!(
                    a.to_bits(),
                    e.to_bits(),
                    "index {i}: got {a} ({:#x}) expected {e} ({:#x})",
                    a.to_bits(),
                    e.to_bits()
                );
            }
        }
    }

    #[test]
    fn rsi_rising_period2_and_flat_all_nan() {
        let got = rsi(&[1.0, 2.0, 3.0, 4.0, 5.0], 2).unwrap();
        assert_bits_eq(&got, &[f64::NAN, f64::NAN, 100.0, 100.0, 100.0]);
        let flat = rsi(&[3.0, 3.0, 3.0, 3.0], 2).unwrap();
        assert!(flat.iter().all(|x| x.is_nan()));
    }

    #[test]
    fn rsi_period1_and_invalid() {
        let got = rsi(&[1.0, 2.0, 4.0, 3.0, 5.0], 1).unwrap();
        assert_bits_eq(&got, &[f64::NAN, 100.0, 100.0, 0.0, 100.0]);
        assert!(matches!(
            rsi(&[1.0], 0),
            Err(IndicatorError::InvalidParameter { .. })
        ));
        assert!(matches!(
            rsi(&[1.0], -2),
            Err(IndicatorError::InvalidParameter { .. })
        ));
    }

    #[test]
    fn atr_period1_and_nan_warmup() {
        let s = [1.0, 2.0, 4.0, 3.0, 5.0];
        let high: Vec<f64> = s.iter().map(|x| x + 1.0).collect();
        let low: Vec<f64> = s.iter().map(|x| x - 1.0).collect();
        let got = atr(&high, &low, &s, 1).unwrap();
        assert_bits_eq(&got, &[2.0, 2.0, 3.0, 2.0, 3.0]);

        let got2 = atr(
            &[f64::NAN, 2.0, 3.0, 4.0],
            &[f64::NAN, 1.0, 2.0, 3.0],
            &[f64::NAN, 1.5, 2.5, 3.5],
            2,
        )
        .unwrap();
        assert_bits_eq(&got2, &[f64::NAN, f64::NAN, 1.25, 1.375]);
    }

    #[test]
    fn macd_nan_prefix_and_span1_zeros() {
        let got = macd(&[f64::NAN, f64::NAN, 1.0, 2.0], 2, 3, 2).unwrap();
        assert_bits_eq(&got.line, &[f64::NAN, f64::NAN, 0.0, 0.16666666666666652]);
        let ones = macd(&[1.0, 2.0, 3.0, 4.0], 1, 1, 1).unwrap();
        assert!(ones
            .line
            .iter()
            .all(|&x| x == 0.0 || x.to_bits() == 0.0_f64.to_bits()));
        assert!(ones
            .histogram
            .iter()
            .all(|&x| x.to_bits() == 0.0_f64.to_bits()));
        assert!(matches!(
            macd(&[1.0], 0, 1, 1),
            Err(IndicatorError::InvalidParameter { .. })
        ));
    }

    #[test]
    fn yang_zhang_window_validation() {
        let o = [1.0, 2.0];
        let h = [1.0, 2.0];
        let l = [1.0, 2.0];
        let c = [1.0, 2.0];
        assert!(matches!(
            yang_zhang(&o, &h, &l, &c, 1, 252),
            Err(IndicatorError::InvalidParameter { .. })
        ));
        let z = yang_zhang(&o, &h, &l, &c, 0, 252).unwrap();
        assert!(z.iter().all(|x| x.is_nan()));
        assert!(matches!(
            yang_zhang(&o, &h, &l, &c, -1, 252),
            Err(IndicatorError::InvalidParameter { .. })
        ));
    }

    #[test]
    fn realized_vol_ppy0_zeros_after_warmup() {
        let got = realized_vol(&[1.0, 2.0, 4.0, 3.0, 5.0], 2, 0).unwrap();
        assert_bits_eq(&got, &[f64::NAN, f64::NAN, 0.0, 0.0, 0.0]);
    }

    #[test]
    fn bollinger_period1_middle_equals_close() {
        let close = [1.0, 2.0, 3.0];
        let bands = bollinger_bands(&close, 1, 2.0).unwrap();
        assert_bits_eq(&bands.middle, &close);
        assert!(bands.upper.iter().all(|x| x.is_nan()));
        assert!(bands.lower.iter().all(|x| x.is_nan()));
    }

    #[test]
    fn donchian_period0_all_nan_and_negative_errors() {
        let high = [1.0, 3.0, 2.0];
        let low = [0.0, 1.0, 1.0];
        let ch = donchian_channels(&high, &low, 0).unwrap();
        assert!(ch.upper.iter().all(|x| x.is_nan()));
        assert!(ch.lower.iter().all(|x| x.is_nan()));
        assert!(matches!(
            donchian_channels(&high, &low, -1),
            Err(IndicatorError::InvalidParameter { .. })
        ));
    }

    #[test]
    fn yang_zhang_and_atr_length_mismatch() {
        let close = [1.0, 2.0, 3.0];
        let high = [1.0, 2.0, 3.0];
        let open = [1.0, 2.0, 3.0];
        let short_low = [1.0, 2.0];
        assert!(matches!(
            yang_zhang(&open, &high, &short_low, &close, 2, 252),
            Err(IndicatorError::LengthMismatch {
                parameter: "low",
                ..
            })
        ));
        assert!(matches!(
            atr(&high, &short_low, &close, 2),
            Err(IndicatorError::LengthMismatch {
                parameter: "low",
                ..
            })
        ));
    }
}
