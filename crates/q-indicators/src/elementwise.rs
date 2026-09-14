//! Elementwise series helpers used by indicator compositions.
//!
//! `pub(crate)` until public indicator wrappers call them.

#![allow(dead_code)]

/// Shift values by `lag` bars, filling the head with NaN.
pub(crate) fn shift(values: &[f64], lag: usize) -> Vec<f64> {
    let n = values.len();
    let mut out = vec![f64::NAN; n];
    if lag >= n {
        return out;
    }
    out[lag..].copy_from_slice(&values[..n - lag]);
    out
}

/// First difference: `out[0] = NaN`, `out[i] = values[i] - values[i - 1]`.
pub(crate) fn diff(values: &[f64]) -> Vec<f64> {
    if values.is_empty() {
        return Vec::new();
    }
    let mut out = vec![f64::NAN; values.len()];
    for i in 1..values.len() {
        out[i] = values[i] - values[i - 1];
    }
    out
}

/// Pandas `Series.clip(lower=low)`: NaN stays NaN; `x >= low ? x : low` keeps `-0.0`.
pub(crate) fn clip_lower(values: &[f64], low: f64) -> Vec<f64> {
    values
        .iter()
        .map(|&x| {
            if x.is_nan() {
                f64::NAN
            } else if x >= low {
                x
            } else {
                low
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shift_lag1() {
        let got = shift(&[1.0, 2.0, 3.0], 1);
        assert!(got[0].is_nan());
        assert_eq!(got[1].to_bits(), 1.0_f64.to_bits());
        assert_eq!(got[2].to_bits(), 2.0_f64.to_bits());
    }

    #[test]
    fn diff_first_nan() {
        let got = diff(&[1.0, 3.0, 2.0]);
        assert!(got[0].is_nan());
        assert_eq!(got[1].to_bits(), 2.0_f64.to_bits());
        assert_eq!(got[2].to_bits(), (-1.0_f64).to_bits());
    }

    #[test]
    fn clip_lower_keeps_negative_zero() {
        let got = clip_lower(&[-0.0], 0.0);
        assert_eq!(got.len(), 1);
        assert!(got[0].is_sign_negative());
        assert_eq!(got[0].to_bits(), (-0.0_f64).to_bits());
    }

    #[test]
    fn clip_lower_keeps_nan() {
        let got = clip_lower(&[f64::NAN, -5.0, 5.0], 0.0);
        assert!(got[0].is_nan());
        assert_eq!(got[1].to_bits(), 0.0_f64.to_bits());
        assert_eq!(got[2].to_bits(), 5.0_f64.to_bits());
    }
}
