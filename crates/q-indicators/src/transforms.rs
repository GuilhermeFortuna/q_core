//! Public transform kernels (z-score, rank, pct_change, clip).

use crate::elementwise::shift;
use crate::error::IndicatorError;
use crate::window::{rolling_mean, rolling_std};

/// Rolling z-score: `(x - mean) / std` on unprepared `x`.
pub fn rolling_zscore(values: &[f64], window: i64) -> Result<Vec<f64>, IndicatorError> {
    if window < 0 {
        return Err(IndicatorError::InvalidParameter {
            function: "rolling_zscore",
            parameter: "window",
            value: window,
            requirement: ">= 0",
        });
    }
    let w = window as usize;
    let mean = rolling_mean(values, w);
    let std = rolling_std(values, w);
    let mut out = vec![f64::NAN; values.len()];
    for i in 0..values.len() {
        out[i] = (values[i] - mean[i]) / std[i];
    }
    Ok(out)
}

/// Rolling percentile rank in `[0, 1]` over a fixed window.
pub fn rolling_rank(values: &[f64], window: i64) -> Result<Vec<f64>, IndicatorError> {
    if values.is_empty() {
        return Ok(Vec::new());
    }
    if window < 1 {
        return Err(IndicatorError::InvalidParameter {
            function: "rolling_rank",
            parameter: "window",
            value: window,
            requirement: ">= 1",
        });
    }
    let w = window as usize;
    let mut out = vec![f64::NAN; values.len()];
    for i in 0..values.len() {
        if i + 1 < w {
            continue;
        }
        let start = i + 1 - w;
        let window_vals = &values[start..=i];
        if window_vals.iter().any(|x| x.is_nan()) {
            continue;
        }
        let current = window_vals[window_vals.len() - 1];
        let count = window_vals.iter().filter(|&&v| v <= current).count();
        out[i] = (count as f64) / (window as f64);
    }
    Ok(out)
}

/// Percent change over `change_bars`: `x / shift(x, lag) - 1`.
pub fn pct_change(values: &[f64], change_bars: i64) -> Result<Vec<f64>, IndicatorError> {
    if change_bars < 1 {
        return Err(IndicatorError::InvalidParameter {
            function: "pct_change",
            parameter: "change_bars",
            value: change_bars,
            requirement: ">= 1",
        });
    }
    let lagged = shift(values, change_bars as usize);
    let mut out = vec![f64::NAN; values.len()];
    for i in 0..values.len() {
        out[i] = values[i] / lagged[i] - 1.0;
    }
    Ok(out)
}

/// Clip values to `[low, high]`, skipping a NaN bound on that side.
pub fn clip(values: &[f64], low: f64, high: f64) -> Result<Vec<f64>, IndicatorError> {
    if low.is_finite() && high.is_finite() && low > high {
        return Err(IndicatorError::InvalidBounds { low, high });
    }
    Ok(values
        .iter()
        .map(|&x| {
            if x.is_nan() {
                return f64::NAN;
            }
            let mut r = x;
            if !low.is_nan() {
                r = if r >= low { r } else { low };
            }
            if !high.is_nan() {
                r = if r <= high { r } else { high };
            }
            r
        })
        .collect())
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
    fn rolling_rank_with_inf() {
        let got = rolling_rank(&[1.0, f64::INFINITY, 2.0, f64::INFINITY], 2).unwrap();
        assert_bits_eq(&got, &[f64::NAN, 1.0, 0.5, 1.0]);
    }

    #[test]
    fn rolling_rank_empty_window0() {
        let got = rolling_rank(&[], 0).unwrap();
        assert!(got.is_empty());
    }

    #[test]
    fn rolling_rank_window_nonpositive_errors() {
        assert!(matches!(
            rolling_rank(&[1.0], 0),
            Err(IndicatorError::InvalidParameter { .. })
        ));
        assert!(matches!(
            rolling_rank(&[1.0], -1),
            Err(IndicatorError::InvalidParameter { .. })
        ));
    }

    #[test]
    fn rolling_zscore_constant_all_nan() {
        let got = rolling_zscore(&[2.0; 5], 3).unwrap();
        assert!(got.iter().all(|x| x.is_nan()));
    }

    #[test]
    fn rolling_zscore_window0_all_nan_and_negative_errors() {
        let got = rolling_zscore(&[1.0, 2.0, 3.0], 0).unwrap();
        assert!(got.iter().all(|x| x.is_nan()));
        assert!(matches!(
            rolling_zscore(&[1.0], -1),
            Err(IndicatorError::InvalidParameter { .. })
        ));
    }

    #[test]
    fn pct_change_matches_plan_and_lag0_errors() {
        let got = pct_change(&[0.0, 1.0, 0.0, 0.0], 1).unwrap();
        assert!(got[0].is_nan());
        assert!(got[1].is_infinite() && got[1].is_sign_positive());
        assert_eq!(got[2].to_bits(), (-1.0_f64).to_bits());
        assert!(got[3].is_nan());
        assert!(matches!(
            pct_change(&[1.0], 0),
            Err(IndicatorError::InvalidParameter { .. })
        ));
    }

    #[test]
    fn clip_nan_and_bounds() {
        let got = clip(&[f64::NAN, -5.0, 5.0, 0.5], 0.0, 1.0).unwrap();
        assert!(got[0].is_nan());
        assert_eq!(got[1].to_bits(), 0.0_f64.to_bits());
        assert_eq!(got[2].to_bits(), 1.0_f64.to_bits());
        assert_eq!(got[3].to_bits(), 0.5_f64.to_bits());
    }

    #[test]
    fn clip_nan_low_skips_lower() {
        let got = clip(&[1.0, 2.0, 4.0, 3.0, 5.0], f64::NAN, 3.0).unwrap();
        assert_bits_eq(&got, &[1.0, 2.0, 3.0, 3.0, 3.0]);
    }

    #[test]
    fn clip_invalid_bounds() {
        assert!(matches!(
            clip(&[1.0], 3.0, 2.0),
            Err(IndicatorError::InvalidBounds {
                low: 3.0,
                high: 2.0
            })
        ));
    }
}
