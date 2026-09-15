//! Exit-rule parameters: Python-style coercion and backend defaults.

use super::pyops::is_missing;

/// A numeric parameter as the caller holds it; coerced with Python's float()/int() rules.
#[derive(Clone, Copy, Debug)]
pub enum ParamValue {
    Bool(bool),
    Int(i64),
    Float(f64),
}

impl ParamValue {
    /// Coerce like Python `float(...)`: Bool → 0.0/1.0, Int as f64.
    pub fn to_float(self) -> f64 {
        todo!("to_float")
    }

    /// Coerce like Python `int(...)`: Float truncates toward zero; NaN/inf rejected.
    pub fn to_int(self, name: &'static str) -> Result<i64, ExitError> {
        let _ = name;
        todo!("to_int")
    }
}

/// Every exit parameter; defaults equal the backend's `params.get(name, default)`.
#[derive(Clone, Debug)]
pub struct ExitParams {
    pub stop_loss_pct: f64,
    pub stop_loss_atr: f64,
    pub take_profit_pct: f64,
    pub take_profit_atr: f64,
    pub trailing_stop_pct: f64,
    pub atr_period: i64,
    pub chandelier_atr_mult: f64,
    pub breakeven_trigger_pct: f64,
    pub breakeven_offset_pct: f64,
    pub psar_af_start: f64,
    pub psar_af_step: f64,
    pub psar_af_max: f64,
    pub target_ratchet_atr: f64,
    pub max_bars_in_trade: i64,
    pub donchian_exit_period: i64,
}

impl Default for ExitParams {
    fn default() -> Self {
        todo!("ExitParams::default")
    }
}

impl ExitParams {
    /// Unknown names are ignored, as the backend ignores non-exit parameters.
    pub fn from_pairs<'a>(
        pairs: impl IntoIterator<Item = (&'a str, ParamValue)>,
    ) -> Result<Self, ExitError> {
        let _ = pairs;
        todo!("from_pairs")
    }
}

/// Construction / input errors for exit evaluation.
#[derive(Clone, Debug, PartialEq)]
pub enum ExitError {
    InvalidParameter {
        name: &'static str,
        reason: &'static str,
    },
    LengthMismatch {
        column: String,
        expected: usize,
        actual: usize,
    },
}

impl core::fmt::Display for ExitError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::InvalidParameter { name, reason } => write!(f, "{name}: {reason}"),
            Self::LengthMismatch {
                column,
                expected,
                actual,
            } => write!(
                f,
                "{column}: length {actual} does not match expected {expected}"
            ),
        }
    }
}

impl std::error::Error for ExitError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_backend() {
        let p = ExitParams::default();
        assert_eq!(p.atr_period, 14);
        assert_eq!(p.psar_af_step.to_bits(), 0.02_f64.to_bits());
        assert_eq!(p.psar_af_max.to_bits(), 0.2_f64.to_bits());
        assert_eq!(p.stop_loss_pct.to_bits(), 0.0_f64.to_bits());
        assert_eq!(p.stop_loss_atr.to_bits(), 0.0_f64.to_bits());
        assert_eq!(p.take_profit_pct.to_bits(), 0.0_f64.to_bits());
        assert_eq!(p.take_profit_atr.to_bits(), 0.0_f64.to_bits());
        assert_eq!(p.trailing_stop_pct.to_bits(), 0.0_f64.to_bits());
        assert_eq!(p.chandelier_atr_mult.to_bits(), 0.0_f64.to_bits());
        assert_eq!(p.breakeven_trigger_pct.to_bits(), 0.0_f64.to_bits());
        assert_eq!(p.breakeven_offset_pct.to_bits(), 0.0_f64.to_bits());
        assert_eq!(p.psar_af_start.to_bits(), 0.0_f64.to_bits());
        assert_eq!(p.target_ratchet_atr.to_bits(), 0.0_f64.to_bits());
        assert_eq!(p.max_bars_in_trade, 0);
        assert_eq!(p.donchian_exit_period, 0);
    }

    #[test]
    fn float_atr_period_truncates_toward_zero() {
        let p = ExitParams::from_pairs([("atr_period", ParamValue::Float(14.7))]).unwrap();
        assert_eq!(p.atr_period, 14);
        let p = ExitParams::from_pairs([("atr_period", ParamValue::Float(-3.9))]).unwrap();
        assert_eq!(p.atr_period, -3);
    }

    #[test]
    fn nan_int_param_is_invalid() {
        let err = ExitParams::from_pairs([("max_bars_in_trade", ParamValue::Float(f64::NAN))])
            .unwrap_err();
        match err {
            ExitError::InvalidParameter { name, .. } => assert_eq!(name, "max_bars_in_trade"),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn bool_true_for_float_param_is_one() {
        let p = ExitParams::from_pairs([("stop_loss_pct", ParamValue::Bool(true))]).unwrap();
        assert_eq!(p.stop_loss_pct.to_bits(), 1.0_f64.to_bits());
    }

    #[test]
    fn unknown_name_is_ignored() {
        let p = ExitParams::from_pairs([("holding_period", ParamValue::Int(10))]).unwrap();
        assert_eq!(p.atr_period, 14);
        assert_eq!(p.stop_loss_pct.to_bits(), 0.0_f64.to_bits());
        assert_eq!(p.max_bars_in_trade, 0);
    }
}
