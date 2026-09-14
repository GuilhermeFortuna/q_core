//! Public moving-average kernels (SMA, EMA, SMMA, WMA, HMA).

use crate::error::IndicatorError;
use crate::ieee::{com_from_alpha_period, com_from_span};
use crate::window::{ewm_mean, rolling_linear_wma, rolling_mean};

fn validate_period(function: &'static str, period: i64) -> Result<(), IndicatorError> {
    if period < 1 {
        return Err(IndicatorError::InvalidParameter {
            function,
            parameter: "period",
            value: period,
            requirement: ">= 1",
        });
    }
    Ok(())
}

/// Simple moving average (`rolling_mean`).
pub fn sma(values: &[f64], period: i64) -> Result<Vec<f64>, IndicatorError> {
    validate_period("sma", period)?;
    Ok(rolling_mean(values, period as usize))
}

/// Exponential moving average (`ewm` with span).
pub fn ema(values: &[f64], period: i64) -> Result<Vec<f64>, IndicatorError> {
    validate_period("ema", period)?;
    Ok(ewm_mean(values, com_from_span(period), 0))
}

/// Smoothed moving average (`ewm` with alpha = 1/period).
pub fn smma(values: &[f64], period: i64) -> Result<Vec<f64>, IndicatorError> {
    validate_period("smma", period)?;
    Ok(ewm_mean(values, com_from_alpha_period(period), 0))
}

/// Linear weighted moving average.
pub fn wma(values: &[f64], period: i64) -> Result<Vec<f64>, IndicatorError> {
    validate_period("wma", period)?;
    Ok(rolling_linear_wma(values, period as usize))
}

/// Hull moving average.
pub fn hma(values: &[f64], period: i64) -> Result<Vec<f64>, IndicatorError> {
    validate_period("hma", period)?;
    let half = (period / 2).max(1);
    let sqrt_p = ((period as f64).sqrt() as i64).max(1);
    let wma_half = rolling_linear_wma(values, half as usize);
    let wma_full = rolling_linear_wma(values, period as usize);
    let mut raw = vec![f64::NAN; values.len()];
    for i in 0..values.len() {
        #[expect(
            clippy::suboptimal_flops,
            reason = "pandas evaluates a*b+c unfused; mul_add changes the result bits"
        )]
        {
            raw[i] = 2.0 * wma_half[i] - wma_full[i];
        }
    }
    Ok(rolling_linear_wma(&raw, sqrt_p as usize))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ieee::ieee_eq;

    fn arange(n: usize) -> Vec<f64> {
        (0..n).map(|i| i as f64).collect()
    }

    fn assert_bits_eq(actual: &[f64], expected: &[f64]) {
        assert_eq!(actual.len(), expected.len(), "length mismatch");
        for (i, (&a, &e)) in actual.iter().zip(expected.iter()).enumerate() {
            if e.is_nan() {
                assert!(a.is_nan(), "index {i}: expected NaN, got {a}");
            } else {
                assert!(
                    ieee_eq(a, e),
                    "index {i}: got {a} ({:#x}) expected {e} ({:#x})",
                    a.to_bits(),
                    e.to_bits()
                );
                assert_eq!(a.to_bits(), e.to_bits(), "index {i} bit mismatch");
            }
        }
    }

    #[test]
    fn hma_arange10_period4() {
        let got = hma(&arange(10), 4).unwrap();
        let mut expected = vec![f64::NAN; 4];
        expected.extend([4.0, 5.0, 6.0, 7.0, 8.0, 9.0]);
        assert_bits_eq(&got, &expected);
    }

    #[test]
    fn hma_arange30_period9_first_valid_at_10() {
        let got = hma(&arange(30), 9).unwrap();
        for (i, &v) in got.iter().enumerate() {
            if i < 10 {
                assert!(v.is_nan(), "index {i} should be NaN");
            } else {
                assert!(!v.is_nan(), "index {i} should be valid");
            }
        }
    }

    #[test]
    fn hma_period1_is_identity() {
        let x = arange(8);
        let got = hma(&x, 1).unwrap();
        assert_bits_eq(&got, &x);
    }

    #[test]
    fn period_zero_invalid_for_all_five() {
        let x = [1.0, 2.0, 3.0];
        for (name, result) in [
            ("sma", sma(&x, 0)),
            ("ema", ema(&x, 0)),
            ("smma", smma(&x, 0)),
            ("wma", wma(&x, 0)),
            ("hma", hma(&x, 0)),
        ] {
            match result {
                Err(IndicatorError::InvalidParameter {
                    function,
                    parameter,
                    value,
                    requirement,
                }) => {
                    assert_eq!(function, name);
                    assert_eq!(parameter, "period");
                    assert_eq!(value, 0);
                    assert_eq!(requirement, ">= 1");
                }
                other => panic!("{name}: expected InvalidParameter, got {other:?}"),
            }
        }
    }
}
