//! Exit-rule parameters: Python-style coercion and backend defaults.

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
        match self {
            Self::Bool(b) => {
                if b {
                    1.0
                } else {
                    0.0
                }
            }
            Self::Int(i) => i as f64,
            Self::Float(f) => f,
        }
    }

    /// Coerce like Python `int(...)`: Float truncates toward zero; NaN/inf rejected.
    pub fn to_int(self, name: &'static str) -> Result<i64, ExitError> {
        match self {
            Self::Bool(b) => Ok(i64::from(b)),
            Self::Int(i) => Ok(i),
            Self::Float(f) => {
                if f.is_nan() || f.is_infinite() {
                    return Err(ExitError::InvalidParameter {
                        name,
                        reason: "cannot convert NaN or infinity to int",
                    });
                }
                Ok(f as i64)
            }
        }
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
        Self {
            stop_loss_pct: 0.0,
            stop_loss_atr: 0.0,
            take_profit_pct: 0.0,
            take_profit_atr: 0.0,
            trailing_stop_pct: 0.0,
            atr_period: 14,
            chandelier_atr_mult: 0.0,
            breakeven_trigger_pct: 0.0,
            breakeven_offset_pct: 0.0,
            psar_af_start: 0.0,
            psar_af_step: 0.02,
            psar_af_max: 0.2,
            target_ratchet_atr: 0.0,
            max_bars_in_trade: 0,
            donchian_exit_period: 0,
        }
    }
}

impl ExitParams {
    /// Unknown names are ignored, as the backend ignores non-exit parameters.
    pub fn from_pairs<'a>(
        pairs: impl IntoIterator<Item = (&'a str, ParamValue)>,
    ) -> Result<Self, ExitError> {
        let mut p = Self::default();
        for (name, value) in pairs {
            match name {
                "stop_loss_pct" => p.stop_loss_pct = value.to_float(),
                "stop_loss_atr" => p.stop_loss_atr = value.to_float(),
                "take_profit_pct" => p.take_profit_pct = value.to_float(),
                "take_profit_atr" => p.take_profit_atr = value.to_float(),
                "trailing_stop_pct" => p.trailing_stop_pct = value.to_float(),
                "atr_period" => p.atr_period = value.to_int("atr_period")?,
                "chandelier_atr_mult" => p.chandelier_atr_mult = value.to_float(),
                "breakeven_trigger_pct" => p.breakeven_trigger_pct = value.to_float(),
                "breakeven_offset_pct" => p.breakeven_offset_pct = value.to_float(),
                "psar_af_start" => p.psar_af_start = value.to_float(),
                "psar_af_step" => p.psar_af_step = value.to_float(),
                "psar_af_max" => p.psar_af_max = value.to_float(),
                "target_ratchet_atr" => p.target_ratchet_atr = value.to_float(),
                "max_bars_in_trade" => {
                    p.max_bars_in_trade = value.to_int("max_bars_in_trade")?;
                }
                "donchian_exit_period" => {
                    p.donchian_exit_period = value.to_int("donchian_exit_period")?;
                }
                _ => {}
            }
        }
        Ok(p)
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
