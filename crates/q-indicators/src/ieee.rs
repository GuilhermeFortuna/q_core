//! Audited IEEE float helpers for pandas-matching window math.

/// C `==` for `f64`. The only place clippy `float_cmp` is allowed in this crate.
///
/// Clippy 1.98 no longer emits `float_cmp` on operator equality, so this is
/// `allow` rather than `expect` (expect would be unfulfilled and fail `-D warnings`).
#[inline]
#[allow(
    clippy::float_cmp,
    reason = "pandas window kernels use C == for same-value and NaN checks"
)]
pub(crate) fn ieee_eq(a: f64, b: f64) -> bool {
    a == b
}

/// Pandas `BaseWindow._prep_values` missing: NaN or ±infinity.
#[inline]
pub(crate) fn window_missing(x: f64) -> bool {
    x.is_nan() || x.is_infinite()
}

/// Center of mass from span: `(span - 1) / 2`.
#[inline]
pub(crate) fn com_from_span(span: i64) -> f64 {
    (span - 1) as f64 / 2.0
}

/// Center of mass from SMMA/RSI-style alpha period: `alpha = 1/period`, then `(1 - alpha) / alpha`.
#[inline]
pub(crate) fn com_from_alpha_period(period: i64) -> f64 {
    let alpha = 1.0 / (period as f64);
    (1.0 - alpha) / alpha
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn com_from_span_three_is_one() {
        assert_eq!(com_from_span(3).to_bits(), 1.0_f64.to_bits());
    }

    #[test]
    fn com_from_alpha_period_two_is_one() {
        assert_eq!(com_from_alpha_period(2).to_bits(), 1.0_f64.to_bits());
    }

    #[test]
    fn com_from_alpha_period_fourteen_matches_python_bits() {
        // Python: (1 - 1/14) / (1/14) == 13.000000000000002
        assert_eq!(com_from_alpha_period(14).to_bits(), 0x402a_0000_0000_0001);
    }

    #[test]
    fn window_missing_detects_inf_and_not_zero() {
        assert!(window_missing(f64::INFINITY));
        assert!(!window_missing(0.0));
    }

    #[test]
    fn ieee_eq_matches_c_equality() {
        assert!(ieee_eq(1.0, 1.0));
        assert!(!ieee_eq(1.0, 2.0));
    }
}
