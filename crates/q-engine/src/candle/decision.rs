//! Section D of the engine loop: what one closed bar queues for the next.

use crate::exits::{
    ExitBook, ExitInputs, ExitParams, ExitRuleId, ExitRuleSet, OpenPosition, PositionKey,
    RuleState, Side,
};

use super::inputs::SignalColumns;

/// One open trade as the decision step reads it.
#[derive(Clone, Copy, Debug)]
pub struct TradeView {
    pub key: PositionKey,
    pub side: Side,
    pub entry_price: f64,
    /// `None` = the entry time is not a bar of the series.
    pub entry_bar: Option<usize>,
}

/// A close queued for the next bar; `rule` is `None` for a strategy or holding exit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct QueuedExit {
    pub key: PositionKey,
    pub rule: Option<ExitRuleId>,
}

/// The single entry a bar may queue.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct QueuedEntry {
    pub side: Side,
    pub strength: f64,
}

/// Everything one closed bar queues.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Decision {
    pub exits: Vec<QueuedExit>,
    pub entry: Option<QueuedEntry>,
}

/// Section D with exit-rule state that persists across calls.
pub struct DecisionStep {
    book: ExitBook,
}

impl DecisionStep {
    pub fn new(exit_params: ExitParams) -> Self {
        Self {
            book: ExitBook::new(ExitRuleSet::new(exit_params)),
        }
    }

    /// Indicator columns the enabled exit rules read.
    pub fn required_columns(&self) -> Vec<String> {
        self.book.rules().required_columns()
    }

    /// `holding_period_bars` is the strategy's declaration, passed per call; the step's own
    /// state is exit-rule state only.
    ///
    /// Mirrors `signal_columns.evaluate_queued_signals`: exit-rule exits per position, then
    /// strategy or holding exits only while the symbol is still open, then at most one entry.
    /// A strategy has one symbol, so the first exit of either kind closes it: Python adds that
    /// symbol to `closed_symbols` and every later trade in the loop is skipped.
    pub fn decide(
        &mut self,
        bar: usize,
        trades: &[TradeView],
        signals: &SignalColumns<'_>,
        exits: &ExitInputs<'_>,
        holding_period_bars: Option<i64>,
    ) -> Decision {
        let positions: Vec<OpenPosition> = trades
            .iter()
            .map(|trade| OpenPosition {
                key: trade.key,
                side: trade.side,
                entry_price: trade.entry_price,
            })
            .collect();

        let rule_exits = self.book.evaluate(&positions, exits, bar);
        let mut queued: Vec<QueuedExit> = rule_exits
            .iter()
            .map(|decision| QueuedExit {
                key: decision.key,
                rule: Some(decision.rule),
            })
            .collect();
        let mut symbol_closed = !queued.is_empty();

        match (holding_period_bars, signals.bar_index) {
            (Some(holding), Some(bar_index)) => {
                let current_bar = bar_index[bar];
                for trade in trades {
                    if symbol_closed {
                        break;
                    }
                    let Some(entry_bar) = trade.entry_bar else {
                        continue;
                    };
                    if current_bar - bar_index[entry_bar] >= holding {
                        queued.push(QueuedExit {
                            key: trade.key,
                            rule: None,
                        });
                        symbol_closed = true;
                    }
                }
            }
            _ => {
                let exit_long = signals.exit_long[bar];
                let exit_short = signals.exit_short[bar];
                for trade in trades {
                    if symbol_closed {
                        break;
                    }
                    let fires = match trade.side {
                        Side::Long => exit_long,
                        Side::Short => exit_short,
                    };
                    if fires {
                        queued.push(QueuedExit {
                            key: trade.key,
                            rule: None,
                        });
                        symbol_closed = true;
                    }
                }
            }
        }

        let entry = match signals.entry[bar] {
            1 => Some(QueuedEntry {
                side: Side::Long,
                strength: signals.strength[bar],
            }),
            -1 => Some(QueuedEntry {
                side: Side::Short,
                strength: signals.strength[bar],
            }),
            _ => None,
        };

        Decision {
            exits: queued,
            entry,
        }
    }

    pub fn state(&self, key: PositionKey) -> Option<&RuleState> {
        self.book.state(key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exits::ParamValue;

    const N: usize = 8;

    struct Series {
        close: Vec<f64>,
        high: Vec<f64>,
        low: Vec<f64>,
        entry: Vec<i8>,
        exit_long: Vec<bool>,
        exit_short: Vec<bool>,
        strength: Vec<f64>,
        bar_index: Vec<i64>,
    }

    impl Series {
        fn flat() -> Self {
            Self {
                close: vec![100.0; N],
                high: vec![100.0; N],
                low: vec![100.0; N],
                entry: vec![0; N],
                exit_long: vec![false; N],
                exit_short: vec![false; N],
                strength: vec![0.0; N],
                bar_index: (0..N as i64).collect(),
            }
        }

        fn signals(&self, with_bar_index: bool) -> SignalColumns<'_> {
            SignalColumns {
                entry: &self.entry,
                exit_long: &self.exit_long,
                exit_short: &self.exit_short,
                strength: &self.strength,
                bar_index: with_bar_index.then_some(&self.bar_index),
            }
        }

        fn exit_inputs<'a>(&'a self, rules: &ExitRuleSet) -> ExitInputs<'a> {
            ExitInputs::from_slices(
                &self.close,
                Some(&self.high),
                Some(&self.low),
                &|_| None,
                rules,
            )
            .expect("hand-built columns are equal length")
        }
    }

    fn params(pairs: &[(&str, ParamValue)]) -> ExitParams {
        ExitParams::from_pairs(pairs.iter().copied()).expect("valid exit parameters")
    }

    fn long(key: u64, entry_bar: Option<usize>) -> TradeView {
        TradeView {
            key: PositionKey(key),
            side: Side::Long,
            entry_price: 100.0,
            entry_bar,
        }
    }

    fn short(key: u64, entry_bar: Option<usize>) -> TradeView {
        TradeView {
            key: PositionKey(key),
            side: Side::Short,
            entry_price: 100.0,
            entry_bar,
        }
    }

    #[test]
    fn long_exit_column_closes_the_long_and_leaves_the_short() {
        let mut series = Series::flat();
        series.exit_long[5] = true;
        let rules = ExitRuleSet::new(params(&[]));
        let exits = series.exit_inputs(&rules);
        let mut step = DecisionStep::new(params(&[]));

        let decision = step.decide(
            5,
            &[long(0, Some(1)), short(1, Some(2))],
            &series.signals(false),
            &exits,
            None,
        );

        assert_eq!(
            decision,
            Decision {
                exits: vec![QueuedExit {
                    key: PositionKey(0),
                    rule: None,
                }],
                entry: None,
            }
        );
    }

    #[test]
    fn a_fired_rule_suppresses_every_strategy_exit_on_the_symbol() {
        let mut series = Series::flat();
        // Long stop at 90 is breached; the short's stop at 110 is not.
        series.low[5] = 85.0;
        series.exit_long[5] = true;
        series.exit_short[5] = true;
        let pairs = [("stop_loss_pct", ParamValue::Float(0.10))];
        let rules = ExitRuleSet::new(params(&pairs));
        let exits = series.exit_inputs(&rules);
        let mut step = DecisionStep::new(params(&pairs));

        let decision = step.decide(
            5,
            &[long(0, Some(1)), short(1, Some(2))],
            &series.signals(false),
            &exits,
            None,
        );

        assert_eq!(
            decision,
            Decision {
                exits: vec![QueuedExit {
                    key: PositionKey(0),
                    rule: Some(ExitRuleId::FixedStopLoss),
                }],
                entry: None,
            },
            "the rule exit is the only close; the short gets no strategy exit"
        );
        assert!(
            step.state(PositionKey(1)).is_some(),
            "the short keeps its rule state across the suppressed bar"
        );
    }

    #[test]
    fn holding_period_closes_two_bars_after_entry_and_ignores_exit_columns() {
        let mut series = Series::flat();
        series.exit_long = vec![true; N];
        let rules = ExitRuleSet::new(params(&[]));
        let exits = series.exit_inputs(&rules);
        let mut step = DecisionStep::new(params(&[]));
        let trades = [long(0, Some(3))];

        let early = step.decide(4, &trades, &series.signals(true), &exits, Some(2));
        assert_eq!(
            early,
            Decision::default(),
            "one bar elapsed is short of the period, and exit_long is ignored while it is set"
        );

        let due = step.decide(5, &trades, &series.signals(true), &exits, Some(2));
        assert_eq!(
            due,
            Decision {
                exits: vec![QueuedExit {
                    key: PositionKey(0),
                    rule: None,
                }],
                entry: None,
            }
        );
    }

    #[test]
    fn a_trade_with_no_entry_bar_is_never_closed_by_the_holding_period() {
        let series = Series::flat();
        let rules = ExitRuleSet::new(params(&[]));
        let exits = series.exit_inputs(&rules);
        let mut step = DecisionStep::new(params(&[]));

        for bar in 0..N {
            let decision = step.decide(
                bar,
                &[long(0, None)],
                &series.signals(true),
                &exits,
                Some(2),
            );
            assert_eq!(
                decision,
                Decision::default(),
                "bar {bar} must queue nothing"
            );
        }
    }

    #[test]
    fn a_short_entry_carries_its_strength() {
        let mut series = Series::flat();
        series.entry[7] = -1;
        series.strength[7] = 0.25;
        let rules = ExitRuleSet::new(params(&[]));
        let exits = series.exit_inputs(&rules);
        let mut step = DecisionStep::new(params(&[]));

        let decision = step.decide(7, &[], &series.signals(false), &exits, None);

        assert_eq!(
            decision,
            Decision {
                exits: Vec::new(),
                entry: Some(QueuedEntry {
                    side: Side::Short,
                    strength: 0.25,
                }),
            }
        );
    }

    #[test]
    fn required_columns_come_from_the_enabled_rules() {
        let step = DecisionStep::new(params(&[
            ("stop_loss_atr", ParamValue::Float(2.0)),
            ("atr_period", ParamValue::Int(21)),
            ("donchian_exit_period", ParamValue::Int(10)),
        ]));
        assert_eq!(
            step.required_columns(),
            vec![
                "atr_21".to_string(),
                "donchian_high_10".to_string(),
                "donchian_low_10".to_string(),
            ]
        );
    }
}
