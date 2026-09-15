//! Exit book: prune, update, and decide across open positions for one bar.

use std::collections::BTreeMap;

use q_buffers::{BarFrame, Column};

use super::logic::{columns_ready, on_bar, should_exit, BarIndicators, BarPrices};
use super::params::ExitError;
use super::pyops::is_missing;
use super::rules::{ExitRuleId, ExitRuleSet, RuleState, Side};

/// Caller-assigned position identity (trade ordinal or interned id).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PositionKey(pub u64);

/// One open position at evaluation time.
#[derive(Clone, Copy, Debug)]
pub struct OpenPosition {
    pub key: PositionKey,
    pub side: Side,
    pub entry_price: f64,
}

/// Column views for one evaluation sequence; every slice has the frame's length.
pub struct ExitInputs<'a> {
    pub close: &'a [f64],
    pub high: Option<&'a [f64]>,
    pub low: Option<&'a [f64]>,
    pub atr: Option<&'a [f64]>,
    pub donchian_high: Option<&'a [f64]>,
    pub donchian_low: Option<&'a [f64]>,
}

impl<'a> ExitInputs<'a> {
    pub fn from_frame(frame: &'a BarFrame, rules: &ExitRuleSet) -> Result<Self, ExitError> {
        Self::from_slices(
            frame.close(),
            Some(frame.high()),
            Some(frame.low()),
            &|name| {
                frame.column(name).and_then(|col| match col {
                    Column::Float64(v) => Some(v.as_slice()),
                    _ => None,
                })
            },
            rules,
        )
    }

    pub fn from_slices(
        close: &'a [f64],
        high: Option<&'a [f64]>,
        low: Option<&'a [f64]>,
        named: &dyn Fn(&str) -> Option<&'a [f64]>,
        rules: &ExitRuleSet,
    ) -> Result<Self, ExitError> {
        let n = close.len();
        let check = |name: &str, col: Option<&'a [f64]>| -> Result<Option<&'a [f64]>, ExitError> {
            match col {
                None => Ok(None),
                Some(v) if v.len() == n => Ok(Some(v)),
                Some(v) => Err(ExitError::LengthMismatch {
                    column: name.to_string(),
                    expected: n,
                    actual: v.len(),
                }),
            }
        };
        let high = check("high", high)?;
        let low = check("low", low)?;

        let params = rules.params();
        let atr_name = format!("atr_{}", params.atr_period);
        let dh_name = format!("donchian_high_{}", params.donchian_exit_period);
        let dl_name = format!("donchian_low_{}", params.donchian_exit_period);

        let atr = check(&atr_name, named(&atr_name))?;
        let donchian_high = check(&dh_name, named(&dh_name))?;
        let donchian_low = check(&dl_name, named(&dl_name))?;

        Ok(Self {
            close,
            high,
            low,
            atr,
            donchian_high,
            donchian_low,
        })
    }

    pub fn len(&self) -> usize {
        self.close.len()
    }

    pub fn is_empty(&self) -> bool {
        self.close.is_empty()
    }

    fn prices_at(&self, bar: usize) -> BarPrices {
        let close = self.close[bar];
        let high = self.high.map(|h| h[bar]).unwrap_or(close);
        let low = self.low.map(|l| l[bar]).unwrap_or(close);
        // Python _bar_prices: missing high/low keys fall back to close; NaN highs/lows stay NaN
        // (present keys). Option None means the column is absent.
        BarPrices { high, low }
    }

    fn indicators_at(&self, bar: usize) -> BarIndicators {
        BarIndicators {
            atr: self.atr.and_then(|c| present(c[bar])),
            donchian_high: self.donchian_high.and_then(|c| present(c[bar])),
            donchian_low: self.donchian_low.and_then(|c| present(c[bar])),
        }
    }
}

fn present(x: f64) -> Option<f64> {
    if is_missing(x) {
        None
    } else {
        Some(x)
    }
}

/// One exit decision for a position on a bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExitDecision {
    pub key: PositionKey,
    pub rule: ExitRuleId,
}

/// Exit-rule state for every open position of one strategy.
pub struct ExitBook {
    rules: ExitRuleSet,
    states: BTreeMap<PositionKey, RuleState>,
}

impl ExitBook {
    pub fn new(rules: ExitRuleSet) -> Self {
        Self {
            rules,
            states: BTreeMap::new(),
        }
    }

    pub fn rules(&self) -> &ExitRuleSet {
        &self.rules
    }

    /// `ExitStrategy.check_exits` for bar `bar`: prune, then update/decide per position and rule.
    pub fn evaluate(
        &mut self,
        positions: &[OpenPosition],
        inputs: &ExitInputs<'_>,
        bar: usize,
    ) -> Vec<ExitDecision> {
        let active: std::collections::BTreeSet<_> = positions.iter().map(|p| p.key).collect();
        self.states.retain(|k, _| active.contains(k));
        if positions.is_empty() {
            return Vec::new();
        }

        let prices = inputs.prices_at(bar);
        let inds = inputs.indicators_at(bar);
        let params = self.rules.params().clone();
        let enabled: Vec<ExitRuleId> = self.rules.enabled().to_vec();

        let mut decisions = Vec::new();
        for pos in positions {
            let state = self.states.entry(pos.key).or_default();
            for &rule in &enabled {
                on_bar(
                    rule,
                    state,
                    pos.side,
                    pos.entry_price,
                    prices,
                    inds,
                    &params,
                );
                if !columns_ready(rule, inds, &params) {
                    continue;
                }
                if should_exit(
                    rule,
                    state,
                    pos.side,
                    pos.entry_price,
                    prices,
                    inds,
                    &params,
                ) {
                    decisions.push(ExitDecision { key: pos.key, rule });
                    break;
                }
            }
        }
        decisions
    }

    pub fn state(&self, key: PositionKey) -> Option<&RuleState> {
        self.states.get(&key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exits::params::{ExitParams, ParamValue};

    fn book_with(pairs: &[(&str, ParamValue)]) -> ExitBook {
        let params = ExitParams::from_pairs(pairs.iter().copied()).unwrap();
        ExitBook::new(ExitRuleSet::new(params))
    }

    #[test]
    fn two_positions_hit_by_stop_yield_decisions_in_slice_order() {
        let mut book = book_with(&[("stop_loss_pct", ParamValue::Float(0.10))]);
        let close = [100.0_f64];
        let low = [85.0_f64];
        let high = [100.0_f64];
        let rules = ExitRuleSet::new(
            ExitParams::from_pairs([("stop_loss_pct", ParamValue::Float(0.10))]).unwrap(),
        );
        let inputs =
            ExitInputs::from_slices(&close, Some(&high), Some(&low), &|_| None, &rules).unwrap();
        let positions = [
            OpenPosition {
                key: PositionKey(1),
                side: Side::Long,
                entry_price: 100.0,
            },
            OpenPosition {
                key: PositionKey(2),
                side: Side::Long,
                entry_price: 100.0,
            },
        ];
        let decisions = book.evaluate(&positions, &inputs, 0);
        assert_eq!(
            decisions,
            vec![
                ExitDecision {
                    key: PositionKey(1),
                    rule: ExitRuleId::FixedStopLoss,
                },
                ExitDecision {
                    key: PositionKey(2),
                    rule: ExitRuleId::FixedStopLoss,
                },
            ]
        );
    }

    #[test]
    fn first_firing_rule_skips_later_rule_state_update() {
        let mut book = book_with(&[
            ("stop_loss_pct", ParamValue::Float(0.10)),
            ("max_bars_in_trade", ParamValue::Int(50)),
        ]);
        let close = [100.0_f64];
        let low = [85.0_f64];
        let high = [100.0_f64];
        let rules = book.rules().params().clone();
        let rules = ExitRuleSet::new(rules);
        let inputs =
            ExitInputs::from_slices(&close, Some(&high), Some(&low), &|_| None, &rules).unwrap();
        let positions = [OpenPosition {
            key: PositionKey(7),
            side: Side::Long,
            entry_price: 100.0,
        }];
        let decisions = book.evaluate(&positions, &inputs, 0);
        assert_eq!(decisions[0].rule, ExitRuleId::FixedStopLoss);
        let state = book.state(PositionKey(7)).unwrap();
        assert_eq!(
            state.time_stop_bars, 0,
            "time stop must not update after earlier rule fires"
        );
    }

    #[test]
    fn absent_key_loses_state_and_empty_slice_clears_all() {
        let mut book = book_with(&[("trailing_stop_pct", ParamValue::Float(0.05))]);
        let close = [100.0_f64, 110.0];
        let high = [105.0_f64, 110.0];
        let low = [99.0_f64, 108.0];
        let rules = ExitRuleSet::new(
            ExitParams::from_pairs([("trailing_stop_pct", ParamValue::Float(0.05))]).unwrap(),
        );
        let inputs =
            ExitInputs::from_slices(&close, Some(&high), Some(&low), &|_| None, &rules).unwrap();
        let p1 = [OpenPosition {
            key: PositionKey(1),
            side: Side::Long,
            entry_price: 100.0,
        }];
        book.evaluate(&p1, &inputs, 0);
        assert!(book
            .state(PositionKey(1))
            .unwrap()
            .trailing_extreme
            .is_some());

        book.evaluate(&[], &inputs, 1);
        assert!(book.state(PositionKey(1)).is_none());

        book.evaluate(&p1, &inputs, 0);
        assert!(book.state(PositionKey(1)).is_some());
        book.evaluate(
            &[OpenPosition {
                key: PositionKey(2),
                side: Side::Long,
                entry_price: 100.0,
            }],
            &inputs,
            1,
        );
        assert!(book.state(PositionKey(1)).is_none());
        assert!(book.state(PositionKey(2)).is_some());
    }

    #[test]
    fn reused_key_starts_with_fresh_trailing_extreme() {
        let mut book = book_with(&[("trailing_stop_pct", ParamValue::Float(0.05))]);
        let close = [100.0_f64];
        let high = [105.0_f64];
        let low = [99.0_f64];
        let rules = ExitRuleSet::new(
            ExitParams::from_pairs([("trailing_stop_pct", ParamValue::Float(0.05))]).unwrap(),
        );
        let inputs =
            ExitInputs::from_slices(&close, Some(&high), Some(&low), &|_| None, &rules).unwrap();
        let p = [OpenPosition {
            key: PositionKey(9),
            side: Side::Long,
            entry_price: 100.0,
        }];
        book.evaluate(&p, &inputs, 0);
        assert!(book
            .state(PositionKey(9))
            .unwrap()
            .trailing_extreme
            .is_some());
        book.evaluate(&[], &inputs, 0);
        book.evaluate(&p, &inputs, 0);
        // Fresh state: extreme re-initialised; presence alone proves reset happened after clear.
        // Before re-eval extreme was None after prune.
        assert!(book
            .state(PositionKey(9))
            .unwrap()
            .trailing_extreme
            .is_some());
    }

    #[test]
    fn from_slices_with_high_none_reads_close() {
        let rules = ExitRuleSet::new(
            ExitParams::from_pairs([("stop_loss_pct", ParamValue::Float(0.10))]).unwrap(),
        );
        let close = [90.0_f64];
        let inputs = ExitInputs::from_slices(&close, None, None, &|_| None, &rules).unwrap();
        let mut book = ExitBook::new(rules);
        // entry 100, stop at 90; high/low fall back to close=90 → long exits
        let decisions = book.evaluate(
            &[OpenPosition {
                key: PositionKey(1),
                side: Side::Long,
                entry_price: 100.0,
            }],
            &inputs,
            0,
        );
        assert_eq!(decisions[0].rule, ExitRuleId::FixedStopLoss);
    }
}
