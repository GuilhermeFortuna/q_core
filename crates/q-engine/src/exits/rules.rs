//! Exit rule identities, enablement, and ordered rule sets.

use super::params::ExitParams;

/// Rule identities in backend registry order; the discriminant is the fixture exit code.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum ExitRuleId {
    FixedStopLoss = 0,
    AtrStopLoss = 1,
    FixedTakeProfit = 2,
    AtrTakeProfit = 3,
    Trailing = 4,
    Chandelier = 5,
    Breakeven = 6,
    ParabolicSar = 7,
    ProfitTargetRatchet = 8,
    TimeStop = 9,
    DonchianStop = 10,
}

impl ExitRuleId {
    pub const REGISTRY_ORDER: [ExitRuleId; 11] = [
        Self::FixedStopLoss,
        Self::AtrStopLoss,
        Self::FixedTakeProfit,
        Self::AtrTakeProfit,
        Self::Trailing,
        Self::Chandelier,
        Self::Breakeven,
        Self::ParabolicSar,
        Self::ProfitTargetRatchet,
        Self::TimeStop,
        Self::DonchianStop,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::FixedStopLoss => "fixed_sl",
            Self::AtrStopLoss => "atr_sl",
            Self::FixedTakeProfit => "fixed_tp",
            Self::AtrTakeProfit => "atr_tp",
            Self::Trailing => "trailing",
            Self::Chandelier => "chandelier",
            Self::Breakeven => "breakeven",
            Self::ParabolicSar => "psar",
            Self::ProfitTargetRatchet => "profit_target_ratchet",
            Self::TimeStop => "time_stop",
            Self::DonchianStop => "donchian_stop",
        }
    }

    pub fn is_enabled(self, params: &ExitParams) -> bool {
        match self {
            Self::FixedStopLoss => params.stop_loss_pct > 0.0,
            Self::AtrStopLoss => params.stop_loss_atr > 0.0,
            Self::FixedTakeProfit => params.take_profit_pct > 0.0,
            Self::AtrTakeProfit => params.take_profit_atr > 0.0,
            Self::Trailing => params.trailing_stop_pct > 0.0,
            Self::Chandelier => params.chandelier_atr_mult > 0.0,
            Self::Breakeven => params.breakeven_trigger_pct > 0.0,
            Self::ParabolicSar => params.psar_af_start > 0.0,
            Self::ProfitTargetRatchet => params.target_ratchet_atr > 0.0,
            Self::TimeStop => params.max_bars_in_trade > 0,
            Self::DonchianStop => params.donchian_exit_period > 0,
        }
    }

    fn required_columns(self, params: &ExitParams) -> Vec<String> {
        if !self.is_enabled(params) {
            return Vec::new();
        }
        match self {
            Self::AtrStopLoss
            | Self::AtrTakeProfit
            | Self::Chandelier
            | Self::ProfitTargetRatchet => {
                vec![format!("atr_{}", params.atr_period)]
            }
            Self::DonchianStop => {
                let p = params.donchian_exit_period;
                vec![format!("donchian_high_{p}"), format!("donchian_low_{p}")]
            }
            _ => Vec::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Long,
    Short,
}

/// Per-rule state for one position; `None` mirrors a key absent from the Python state dict.
#[derive(Clone, Debug, Default)]
pub struct RuleState {
    pub trailing_extreme: Option<f64>,
    pub chandelier_peak: Option<f64>,
    pub breakeven_armed: bool,
    pub psar: Option<PsarState>,
    /// `Some` iff the ratchet is armed.
    pub ratchet: Option<f64>,
    /// 0 until the first time-stop update.
    pub time_stop_bars: i64,
}

/// Parabolic SAR recursion state (lows for long, highs for short).
#[derive(Clone, Copy, Debug)]
pub struct PsarState {
    pub sar: f64,
    pub ep: f64,
    pub af: f64,
    pub prior: f64,
    pub prior_prior: Option<f64>,
}

/// Ordered set of enabled exit rules for one strategy parameter set.
pub struct ExitRuleSet {
    params: ExitParams,
    enabled: Vec<ExitRuleId>,
}

impl ExitRuleSet {
    pub fn new(params: ExitParams) -> Self {
        let enabled: Vec<ExitRuleId> = ExitRuleId::REGISTRY_ORDER
            .into_iter()
            .filter(|id| id.is_enabled(&params))
            .collect();
        Self { params, enabled }
    }

    pub fn params(&self) -> &ExitParams {
        &self.params
    }

    pub fn enabled(&self) -> &[ExitRuleId] {
        &self.enabled
    }

    /// Sorted, de-duplicated indicator column names required by enabled rules.
    pub fn required_columns(&self) -> Vec<String> {
        let mut cols: Vec<String> = self
            .enabled
            .iter()
            .flat_map(|id| id.required_columns(&self.params))
            .collect();
        cols.sort();
        cols.dedup();
        cols
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exits::params::{ExitParams, ParamValue};

    #[test]
    fn registry_order_matches_backend_ids() {
        let expected = [
            "fixed_sl",
            "atr_sl",
            "fixed_tp",
            "atr_tp",
            "trailing",
            "chandelier",
            "breakeven",
            "psar",
            "profit_target_ratchet",
            "time_stop",
            "donchian_stop",
        ];
        let got: Vec<_> = ExitRuleId::REGISTRY_ORDER
            .iter()
            .map(|id| id.as_str())
            .collect();
        assert_eq!(got, expected);
    }

    #[test]
    fn only_trailing_enabled_when_sole_param() {
        let params =
            ExitParams::from_pairs([("trailing_stop_pct", ParamValue::Float(0.03))]).unwrap();
        let set = ExitRuleSet::new(params);
        assert_eq!(set.enabled(), &[ExitRuleId::Trailing]);
    }

    #[test]
    fn all_eleven_enabled_preserves_registry_order() {
        let params = ExitParams::from_pairs([
            ("stop_loss_pct", ParamValue::Float(0.02)),
            ("stop_loss_atr", ParamValue::Float(2.0)),
            ("take_profit_pct", ParamValue::Float(0.05)),
            ("take_profit_atr", ParamValue::Float(3.0)),
            ("trailing_stop_pct", ParamValue::Float(0.02)),
            ("chandelier_atr_mult", ParamValue::Float(3.0)),
            ("breakeven_trigger_pct", ParamValue::Float(0.02)),
            ("psar_af_start", ParamValue::Float(0.02)),
            ("target_ratchet_atr", ParamValue::Float(2.0)),
            ("max_bars_in_trade", ParamValue::Int(50)),
            ("donchian_exit_period", ParamValue::Int(20)),
        ])
        .unwrap();
        let set = ExitRuleSet::new(params);
        assert_eq!(set.enabled(), &ExitRuleId::REGISTRY_ORDER[..]);
    }

    #[test]
    fn required_columns_sorted_and_deduped() {
        let params = ExitParams::from_pairs([
            ("stop_loss_atr", ParamValue::Float(2.0)),
            ("chandelier_atr_mult", ParamValue::Float(3.0)),
            ("atr_period", ParamValue::Int(21)),
            ("donchian_exit_period", ParamValue::Int(10)),
        ])
        .unwrap();
        let set = ExitRuleSet::new(params);
        assert_eq!(
            set.required_columns(),
            vec![
                "atr_21".to_string(),
                "donchian_high_10".to_string(),
                "donchian_low_10".to_string(),
            ]
        );
    }
}
