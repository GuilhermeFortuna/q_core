//! Per-rule update and decide, matching the backend exit_rules modules bit for bit.

#![allow(
    dead_code,
    reason = "ExitBook (step 6) is the non-test caller of these rule paths"
)]

use super::params::ExitParams;
use super::pyops::{py_max, py_min};
use super::rules::{ExitRuleId, PsarState, RuleState, Side};

/// Resolved OHLC prices for one bar (`high`/`low` already fall back to close).
#[derive(Clone, Copy, Debug)]
pub(crate) struct BarPrices {
    pub high: f64,
    pub low: f64,
}

/// Indicator values for one bar; `None` means the column is absent or NaN.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct BarIndicators {
    pub atr: Option<f64>,
    pub donchian_high: Option<f64>,
    pub donchian_low: Option<f64>,
}

pub(crate) fn columns_ready(rule: ExitRuleId, inds: BarIndicators, params: &ExitParams) -> bool {
    match rule {
        ExitRuleId::AtrStopLoss
        | ExitRuleId::AtrTakeProfit
        | ExitRuleId::Chandelier
        | ExitRuleId::ProfitTargetRatchet => inds.atr.is_some(),
        ExitRuleId::DonchianStop => {
            if params.donchian_exit_period <= 0 {
                return true;
            }
            inds.donchian_high.is_some() && inds.donchian_low.is_some()
        }
        _ => true,
    }
}

pub(crate) fn on_bar(
    rule: ExitRuleId,
    state: &mut RuleState,
    side: Side,
    entry: f64,
    prices: BarPrices,
    inds: BarIndicators,
    params: &ExitParams,
) {
    match rule {
        ExitRuleId::Trailing => update_trailing(state, side, entry, prices),
        ExitRuleId::Chandelier => update_chandelier(state, side, entry, prices),
        ExitRuleId::Breakeven => update_breakeven(state, side, entry, prices, params),
        ExitRuleId::ParabolicSar => update_psar(state, side, entry, prices, params),
        ExitRuleId::ProfitTargetRatchet => {
            update_ratchet(state, side, entry, prices, inds.atr, params);
        }
        ExitRuleId::TimeStop => {
            state.time_stop_bars += 1;
        }
        _ => {}
    }
}

pub(crate) fn should_exit(
    rule: ExitRuleId,
    state: &RuleState,
    side: Side,
    entry: f64,
    prices: BarPrices,
    inds: BarIndicators,
    params: &ExitParams,
) -> bool {
    match rule {
        ExitRuleId::FixedStopLoss => fixed_sl(side, entry, prices, params),
        ExitRuleId::AtrStopLoss => atr_sl(side, entry, prices, inds.atr, params),
        ExitRuleId::FixedTakeProfit => fixed_tp(side, entry, prices, params),
        ExitRuleId::AtrTakeProfit => atr_tp(side, entry, prices, inds.atr, params),
        ExitRuleId::Trailing => trailing_exit(state, side, entry, prices, params),
        ExitRuleId::Chandelier => chandelier_exit(state, side, entry, prices, inds.atr, params),
        ExitRuleId::Breakeven => breakeven_exit(state, side, entry, prices, params),
        ExitRuleId::ParabolicSar => psar_exit(state, side, prices),
        ExitRuleId::ProfitTargetRatchet => ratchet_exit(state, side, prices),
        ExitRuleId::TimeStop => state.time_stop_bars >= params.max_bars_in_trade,
        ExitRuleId::DonchianStop => donchian_exit(side, prices, inds),
    }
}

#[inline]
#[allow(
    clippy::suboptimal_flops,
    reason = "exact parity with Python: entry * (1.0 ± pct) must not fuse"
)]
fn fixed_sl(side: Side, entry: f64, prices: BarPrices, params: &ExitParams) -> bool {
    let pct = params.stop_loss_pct;
    match side {
        Side::Long => {
            let level = entry * (1.0 - pct);
            prices.low <= level
        }
        Side::Short => {
            let level = entry * (1.0 + pct);
            prices.high >= level
        }
    }
}

#[inline]
#[allow(
    clippy::suboptimal_flops,
    reason = "exact parity with Python: entry ± (mult * atr) must not fuse"
)]
fn atr_sl(
    side: Side,
    entry: f64,
    prices: BarPrices,
    atr: Option<f64>,
    params: &ExitParams,
) -> bool {
    let Some(atr_val) = atr else {
        return false;
    };
    let mult = params.stop_loss_atr;
    match side {
        Side::Long => {
            let level = entry - (mult * atr_val);
            prices.low <= level
        }
        Side::Short => {
            let level = entry + (mult * atr_val);
            prices.high >= level
        }
    }
}

#[inline]
#[allow(
    clippy::suboptimal_flops,
    reason = "exact parity with Python: entry * (1.0 ± pct) must not fuse"
)]
fn fixed_tp(side: Side, entry: f64, prices: BarPrices, params: &ExitParams) -> bool {
    let pct = params.take_profit_pct;
    match side {
        Side::Long => {
            let level = entry * (1.0 + pct);
            prices.high >= level
        }
        Side::Short => {
            let level = entry * (1.0 - pct);
            prices.low <= level
        }
    }
}

#[inline]
#[allow(
    clippy::suboptimal_flops,
    reason = "exact parity with Python: entry ± (mult * atr) must not fuse"
)]
fn atr_tp(
    side: Side,
    entry: f64,
    prices: BarPrices,
    atr: Option<f64>,
    params: &ExitParams,
) -> bool {
    let Some(atr_val) = atr else {
        return false;
    };
    let mult = params.take_profit_atr;
    match side {
        Side::Long => {
            let level = entry + (mult * atr_val);
            prices.high >= level
        }
        Side::Short => {
            let level = entry - (mult * atr_val);
            prices.low <= level
        }
    }
}

fn update_trailing(state: &mut RuleState, side: Side, entry: f64, prices: BarPrices) {
    match state.trailing_extreme {
        None => {
            state.trailing_extreme = Some(match side {
                Side::Long => py_max(entry, prices.high),
                Side::Short => py_min(entry, prices.low),
            });
        }
        Some(extreme) => {
            state.trailing_extreme = Some(match side {
                Side::Long => py_max(extreme, prices.high),
                Side::Short => py_min(extreme, prices.low),
            });
        }
    }
}

#[inline]
#[allow(
    clippy::suboptimal_flops,
    reason = "exact parity with Python: extreme * (1.0 ± pct) must not fuse"
)]
fn trailing_exit(
    state: &RuleState,
    side: Side,
    entry: f64,
    prices: BarPrices,
    params: &ExitParams,
) -> bool {
    let pct = params.trailing_stop_pct;
    match side {
        Side::Long => {
            let highest = state.trailing_extreme.unwrap_or(entry);
            let trail = highest * (1.0 - pct);
            prices.low <= trail
        }
        Side::Short => {
            let lowest = state.trailing_extreme.unwrap_or(entry);
            let trail = lowest * (1.0 + pct);
            prices.high >= trail
        }
    }
}

fn update_chandelier(state: &mut RuleState, side: Side, entry: f64, prices: BarPrices) {
    let peak = match state.chandelier_peak {
        None => entry,
        Some(p) => p,
    };
    state.chandelier_peak = Some(match side {
        Side::Long => py_max(peak, prices.high),
        Side::Short => py_min(peak, prices.low),
    });
}

#[inline]
#[allow(
    clippy::suboptimal_flops,
    reason = "exact parity with Python: peak ± (mult * atr) must not fuse"
)]
fn chandelier_exit(
    state: &RuleState,
    side: Side,
    entry: f64,
    prices: BarPrices,
    atr: Option<f64>,
    params: &ExitParams,
) -> bool {
    let Some(atr_val) = atr else {
        return false;
    };
    let mult = params.chandelier_atr_mult;
    let peak = state.chandelier_peak.unwrap_or(entry);
    match side {
        Side::Long => {
            let stop = peak - (mult * atr_val);
            prices.low <= stop
        }
        Side::Short => {
            let stop = peak + (mult * atr_val);
            prices.high >= stop
        }
    }
}

#[inline]
#[allow(
    clippy::suboptimal_flops,
    reason = "exact parity with Python: (high - entry) / entry must not fuse"
)]
fn update_breakeven(
    state: &mut RuleState,
    side: Side,
    entry: f64,
    prices: BarPrices,
    params: &ExitParams,
) {
    if state.breakeven_armed {
        return;
    }
    let trigger = params.breakeven_trigger_pct;
    let gain = match side {
        Side::Long => (prices.high - entry) / entry,
        Side::Short => (entry - prices.low) / entry,
    };
    if gain >= trigger {
        state.breakeven_armed = true;
    }
}

#[inline]
#[allow(
    clippy::suboptimal_flops,
    reason = "exact parity with Python: entry * (1.0 ± offset) must not fuse"
)]
fn breakeven_exit(
    state: &RuleState,
    side: Side,
    entry: f64,
    prices: BarPrices,
    params: &ExitParams,
) -> bool {
    if !state.breakeven_armed {
        return false;
    }
    let offset = params.breakeven_offset_pct;
    match side {
        Side::Long => {
            let stop = entry * (1.0 + offset);
            prices.low <= stop
        }
        Side::Short => {
            let stop = entry * (1.0 - offset);
            prices.high >= stop
        }
    }
}

#[inline]
#[allow(
    clippy::suboptimal_flops,
    reason = "exact parity with Python: sar + af * (ep - sar) must not fuse"
)]
fn update_psar(
    state: &mut RuleState,
    side: Side,
    entry: f64,
    prices: BarPrices,
    params: &ExitParams,
) {
    let af_start = params.psar_af_start;
    let af_step = params.psar_af_step;
    let af_max = params.psar_af_max;
    match state.psar {
        None => {
            state.psar = Some(match side {
                Side::Long => PsarState {
                    sar: py_min(entry, prices.low),
                    ep: py_max(entry, prices.high),
                    af: af_start,
                    prior: prices.low,
                    prior_prior: None,
                },
                Side::Short => PsarState {
                    sar: py_max(entry, prices.high),
                    ep: py_min(entry, prices.low),
                    af: af_start,
                    prior: prices.high,
                    prior_prior: None,
                },
            });
        }
        Some(mut ps) => {
            ps.sar += ps.af * (ps.ep - ps.sar);
            match side {
                Side::Long => {
                    ps.sar = py_min(ps.sar, ps.prior);
                    if let Some(pp) = ps.prior_prior {
                        ps.sar = py_min(ps.sar, pp);
                    }
                    if prices.high > ps.ep {
                        ps.ep = prices.high;
                        ps.af = py_min(ps.af + af_step, af_max);
                    }
                    ps.prior_prior = Some(ps.prior);
                    ps.prior = prices.low;
                }
                Side::Short => {
                    ps.sar = py_max(ps.sar, ps.prior);
                    if let Some(pp) = ps.prior_prior {
                        ps.sar = py_max(ps.sar, pp);
                    }
                    if prices.low < ps.ep {
                        ps.ep = prices.low;
                        ps.af = py_min(ps.af + af_step, af_max);
                    }
                    ps.prior_prior = Some(ps.prior);
                    ps.prior = prices.high;
                }
            }
            state.psar = Some(ps);
        }
    }
}

fn psar_exit(state: &RuleState, side: Side, prices: BarPrices) -> bool {
    let Some(ps) = state.psar else {
        return false;
    };
    match side {
        Side::Long => prices.low <= ps.sar,
        Side::Short => prices.high >= ps.sar,
    }
}

#[inline]
#[allow(
    clippy::suboptimal_flops,
    reason = "exact parity with Python: entry ± (mult * atr) and high - (mult * atr) must not fuse"
)]
fn update_ratchet(
    state: &mut RuleState,
    side: Side,
    entry: f64,
    prices: BarPrices,
    atr: Option<f64>,
    params: &ExitParams,
) {
    let Some(atr_val) = atr else {
        return;
    };
    let mult = params.target_ratchet_atr;
    match side {
        Side::Long => {
            let arm_level = entry + (mult * atr_val);
            match state.ratchet {
                None => {
                    if prices.high >= arm_level {
                        state.ratchet = Some(prices.high - (mult * atr_val));
                    }
                }
                Some(r) => {
                    state.ratchet = Some(py_max(r, prices.high - (mult * atr_val)));
                }
            }
        }
        Side::Short => {
            let arm_level = entry - (mult * atr_val);
            match state.ratchet {
                None => {
                    if prices.low <= arm_level {
                        state.ratchet = Some(prices.low + (mult * atr_val));
                    }
                }
                Some(r) => {
                    state.ratchet = Some(py_min(r, prices.low + (mult * atr_val)));
                }
            }
        }
    }
}

fn ratchet_exit(state: &RuleState, side: Side, prices: BarPrices) -> bool {
    let Some(ratchet) = state.ratchet else {
        return false;
    };
    match side {
        Side::Long => prices.low <= ratchet,
        Side::Short => prices.high >= ratchet,
    }
}

fn donchian_exit(side: Side, prices: BarPrices, inds: BarIndicators) -> bool {
    let (Some(dh), Some(dl)) = (inds.donchian_high, inds.donchian_low) else {
        return false;
    };
    match side {
        Side::Long => prices.low <= dl,
        Side::Short => prices.high >= dh,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exits::params::{ExitParams, ParamValue};

    fn params(pairs: &[(&str, ParamValue)]) -> ExitParams {
        ExitParams::from_pairs(pairs.iter().copied()).unwrap()
    }

    fn prices(high: f64, low: f64) -> BarPrices {
        BarPrices { high, low }
    }

    #[test]
    fn fixed_sl_exits_at_exact_boundary_long_and_short() {
        let p = params(&[("stop_loss_pct", ParamValue::Float(0.125))]);
        let entry = 100.0;
        // long: level = 87.5; low == 87.5 exits
        assert!(should_exit(
            ExitRuleId::FixedStopLoss,
            &RuleState::default(),
            Side::Long,
            entry,
            prices(101.0, 87.5),
            BarIndicators::default(),
            &p
        ));
        assert!(!should_exit(
            ExitRuleId::FixedStopLoss,
            &RuleState::default(),
            Side::Long,
            entry,
            prices(101.0, 87.6),
            BarIndicators::default(),
            &p
        ));
        // short: level = 112.5; high == 112.5 exits
        assert!(should_exit(
            ExitRuleId::FixedStopLoss,
            &RuleState::default(),
            Side::Short,
            entry,
            prices(112.5, 99.0),
            BarIndicators::default(),
            &p
        ));
    }

    #[test]
    fn atr_sl_and_tp_at_exact_boundary() {
        let p = params(&[
            ("stop_loss_atr", ParamValue::Float(2.0)),
            ("take_profit_atr", ParamValue::Float(3.0)),
            ("atr_period", ParamValue::Int(14)),
        ]);
        let entry = 100.0;
        let atr = Some(5.0);
        // long SL: 100 - 10 = 90
        assert!(should_exit(
            ExitRuleId::AtrStopLoss,
            &RuleState::default(),
            Side::Long,
            entry,
            prices(101.0, 90.0),
            BarIndicators {
                atr,
                ..Default::default()
            },
            &p
        ));
        // long TP: 100 + 15 = 115
        assert!(should_exit(
            ExitRuleId::AtrTakeProfit,
            &RuleState::default(),
            Side::Long,
            entry,
            prices(115.0, 100.0),
            BarIndicators {
                atr,
                ..Default::default()
            },
            &p
        ));
        // short SL: 100 + 10 = 110
        assert!(should_exit(
            ExitRuleId::AtrStopLoss,
            &RuleState::default(),
            Side::Short,
            entry,
            prices(110.0, 99.0),
            BarIndicators {
                atr,
                ..Default::default()
            },
            &p
        ));
    }

    #[test]
    fn trailing_initialises_to_max_entry_high_then_trails() {
        let p = params(&[("trailing_stop_pct", ParamValue::Float(0.10))]);
        let mut state = RuleState::default();
        on_bar(
            ExitRuleId::Trailing,
            &mut state,
            Side::Long,
            100.0,
            prices(105.0, 99.0),
            BarIndicators::default(),
            &p,
        );
        assert_eq!(
            state.trailing_extreme.unwrap().to_bits(),
            105.0_f64.to_bits()
        );
        on_bar(
            ExitRuleId::Trailing,
            &mut state,
            Side::Long,
            100.0,
            prices(110.0, 104.0),
            BarIndicators::default(),
            &p,
        );
        assert_eq!(
            state.trailing_extreme.unwrap().to_bits(),
            110.0_f64.to_bits()
        );
        // trail at 99; low 99 exits
        assert!(should_exit(
            ExitRuleId::Trailing,
            &state,
            Side::Long,
            100.0,
            prices(100.0, 99.0),
            BarIndicators::default(),
            &p
        ));
    }

    #[test]
    fn chandelier_updates_peak_on_nan_atr_but_does_not_exit() {
        let p = params(&[("chandelier_atr_mult", ParamValue::Float(2.0))]);
        let mut state = RuleState::default();
        on_bar(
            ExitRuleId::Chandelier,
            &mut state,
            Side::Long,
            100.0,
            prices(108.0, 99.0),
            BarIndicators {
                atr: None,
                ..Default::default()
            },
            &p,
        );
        assert_eq!(
            state.chandelier_peak.unwrap().to_bits(),
            108.0_f64.to_bits()
        );
        assert!(!should_exit(
            ExitRuleId::Chandelier,
            &state,
            Side::Long,
            100.0,
            prices(108.0, 90.0),
            BarIndicators {
                atr: None,
                ..Default::default()
            },
            &p
        ));
        // with atr=5, stop = 108 - 10 = 98
        assert!(should_exit(
            ExitRuleId::Chandelier,
            &state,
            Side::Long,
            100.0,
            prices(108.0, 98.0),
            BarIndicators {
                atr: Some(5.0),
                ..Default::default()
            },
            &p
        ));
    }

    #[test]
    fn breakeven_arms_then_exits_at_entry_plus_offset() {
        let p = params(&[
            ("breakeven_trigger_pct", ParamValue::Float(0.05)),
            ("breakeven_offset_pct", ParamValue::Float(0.01)),
        ]);
        let mut state = RuleState::default();
        let entry = 100.0;
        // bar 0: gain 0.04 < 0.05
        on_bar(
            ExitRuleId::Breakeven,
            &mut state,
            Side::Long,
            entry,
            prices(104.0, 100.0),
            BarIndicators::default(),
            &p,
        );
        assert!(!state.breakeven_armed);
        // bar 1: gain 0.05 arms
        on_bar(
            ExitRuleId::Breakeven,
            &mut state,
            Side::Long,
            entry,
            prices(105.0, 100.0),
            BarIndicators::default(),
            &p,
        );
        assert!(state.breakeven_armed);
        // bar 2: stop at 101; low 101 exits
        assert!(should_exit(
            ExitRuleId::Breakeven,
            &state,
            Side::Long,
            entry,
            prices(102.0, 101.0),
            BarIndicators::default(),
            &p
        ));
    }

    #[test]
    fn psar_first_bar_initialises_only_then_clamps_and_caps_af() {
        let p = params(&[
            ("psar_af_start", ParamValue::Float(0.02)),
            ("psar_af_step", ParamValue::Float(0.10)),
            ("psar_af_max", ParamValue::Float(0.25)),
        ]);
        let mut state = RuleState::default();
        let entry = 100.0;
        on_bar(
            ExitRuleId::ParabolicSar,
            &mut state,
            Side::Long,
            entry,
            prices(105.0, 101.0),
            BarIndicators::default(),
            &p,
        );
        let ps = state.psar.unwrap();
        // sar = min(100, 101) = 100; low 101 > sar so first bar does not exit
        assert_eq!(ps.sar.to_bits(), 100.0_f64.to_bits());
        assert_eq!(ps.ep.to_bits(), 105.0_f64.to_bits());
        assert_eq!(ps.af.to_bits(), 0.02_f64.to_bits());
        assert!(!should_exit(
            ExitRuleId::ParabolicSar,
            &state,
            Side::Long,
            entry,
            prices(105.0, 101.0),
            BarIndicators::default(),
            &p
        ));

        // bar 1: advance, no new EP
        on_bar(
            ExitRuleId::ParabolicSar,
            &mut state,
            Side::Long,
            entry,
            prices(104.0, 102.0),
            BarIndicators::default(),
            &p,
        );
        // bar 2: prior_prior becomes available for clamp
        on_bar(
            ExitRuleId::ParabolicSar,
            &mut state,
            Side::Long,
            entry,
            prices(106.0, 103.0),
            BarIndicators::default(),
            &p,
        );
        let ps = state.psar.unwrap();
        assert!(ps.prior_prior.is_some());
        // drive AF to cap with new highs
        on_bar(
            ExitRuleId::ParabolicSar,
            &mut state,
            Side::Long,
            entry,
            prices(110.0, 101.0),
            BarIndicators::default(),
            &p,
        );
        on_bar(
            ExitRuleId::ParabolicSar,
            &mut state,
            Side::Long,
            entry,
            prices(120.0, 102.0),
            BarIndicators::default(),
            &p,
        );
        on_bar(
            ExitRuleId::ParabolicSar,
            &mut state,
            Side::Long,
            entry,
            prices(130.0, 103.0),
            BarIndicators::default(),
            &p,
        );
        assert_eq!(state.psar.unwrap().af.to_bits(), 0.25_f64.to_bits());
    }

    #[test]
    fn ratchet_does_not_arm_on_nan_atr_even_when_high_passes_level() {
        let p = params(&[("target_ratchet_atr", ParamValue::Float(2.0))]);
        let mut state = RuleState::default();
        on_bar(
            ExitRuleId::ProfitTargetRatchet,
            &mut state,
            Side::Long,
            100.0,
            prices(200.0, 100.0),
            BarIndicators {
                atr: None,
                ..Default::default()
            },
            &p,
        );
        assert!(state.ratchet.is_none());
        on_bar(
            ExitRuleId::ProfitTargetRatchet,
            &mut state,
            Side::Long,
            100.0,
            prices(120.0, 100.0),
            BarIndicators {
                atr: Some(5.0),
                ..Default::default()
            },
            &p,
        );
        // arm_level = 100 + 10 = 110; high 120 arms; ratchet = 120 - 10 = 110
        assert_eq!(state.ratchet.unwrap().to_bits(), 110.0_f64.to_bits());
    }

    #[test]
    fn time_stop_exits_on_third_evaluation_when_max_is_three() {
        let p = params(&[("max_bars_in_trade", ParamValue::Int(3))]);
        let mut state = RuleState::default();
        for i in 1..=3 {
            on_bar(
                ExitRuleId::TimeStop,
                &mut state,
                Side::Long,
                100.0,
                prices(101.0, 99.0),
                BarIndicators::default(),
                &p,
            );
            let exit = should_exit(
                ExitRuleId::TimeStop,
                &state,
                Side::Long,
                100.0,
                prices(101.0, 99.0),
                BarIndicators::default(),
                &p,
            );
            assert_eq!(exit, i >= 3, "bar {i}");
        }
    }

    #[test]
    fn donchian_with_both_columns_absent_never_exits() {
        let p = params(&[("donchian_exit_period", ParamValue::Int(20))]);
        assert!(!should_exit(
            ExitRuleId::DonchianStop,
            &RuleState::default(),
            Side::Long,
            100.0,
            prices(101.0, 50.0),
            BarIndicators::default(),
            &p
        ));
        assert!(!columns_ready(
            ExitRuleId::DonchianStop,
            BarIndicators::default(),
            &p
        ));
    }
}
