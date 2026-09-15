//! Audited Python operand helpers for exit-rule float math.
//!
//! Python's `max(a, b)` / `min(a, b)` return the left operand unless the right
//! strictly wins. Rust's `f64::max` / `f64::min` prefer the non-NaN operand, so
//! they diverge whenever a high or low is NaN. Exit rules must match Python.

/// Python `max(a, b)`: `b if b > a else a`.
#[inline]
pub(crate) fn py_max(a: f64, b: f64) -> f64 {
    if b > a {
        b
    } else {
        a
    }
}

/// Python `min(a, b)`: `b if b < a else a`.
#[inline]
pub(crate) fn py_min(a: f64, b: f64) -> f64 {
    if b < a {
        b
    } else {
        a
    }
}

/// `pd.isna` on a float: NaN only (`inf` is present).
#[inline]
pub(crate) fn is_missing(x: f64) -> bool {
    x.is_nan()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn py_max_nan_left_stays_nan() {
        assert!(py_max(f64::NAN, 1.0).is_nan());
    }

    #[test]
    fn py_max_nan_right_keeps_left() {
        assert_eq!(py_max(1.0, f64::NAN).to_bits(), 1.0_f64.to_bits());
    }

    #[test]
    fn py_min_nan_left_stays_nan() {
        assert!(py_min(f64::NAN, 1.0).is_nan());
    }

    #[test]
    fn py_max_neg_zero_vs_pos_zero_keeps_left() {
        // Python: max(-0.0, 0.0) is -0.0 because 0.0 > -0.0 is False.
        assert_eq!(py_max(-0.0, 0.0).to_bits(), (-0.0_f64).to_bits());
    }

    #[test]
    fn is_missing_inf_is_present() {
        assert!(!is_missing(f64::INFINITY));
        assert!(!is_missing(f64::NEG_INFINITY));
        assert!(is_missing(f64::NAN));
        assert!(!is_missing(0.0));
    }
}
