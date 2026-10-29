#![forbid(unsafe_code)]

use q_engine::{
    run_candle_with_callbacks, BarSignals, CandleConfig, CandleError, CandleInputs, CandleRun,
    EntryCandidate, ExitReason, IntrabarPrices, IntrabarSource, PositionSnapshot, RestingKind,
    RestingOrder, RuntimeCallbacks, Side, SignalColumns, Sizing, StrategyFn,
};

struct Tape(Vec<Vec<f64>>);

impl IntrabarSource for Tape {
    fn prices(&self, bar: usize) -> Result<IntrabarPrices, String> {
        let price = self.0[bar].clone();
        Ok(IntrabarPrices {
            time_us: (0..price.len())
                .map(|i| bar as i64 * 10 + i as i64)
                .collect(),
            price,
        })
    }
}

type Decide = Box<dyn FnMut(usize, &[PositionSnapshot]) -> Result<BarSignals, CandleError>>;

/// Runs one bar per tape, each bar's open, high and low taken from its tape.
fn run(tapes: &[&[f64]], mut strategy: Decide, entry: Option<Decide>) -> CandleRun {
    let tape = Tape(tapes.iter().map(|prices| prices.to_vec()).collect());
    let n = tapes.len();
    let time: Vec<i64> = (0..n as i64).map(|bar| bar * 10).collect();
    let open: Vec<f64> = tapes.iter().map(|prices| prices[0]).collect();
    let high: Vec<f64> = tapes
        .iter()
        .map(|prices| prices.iter().copied().fold(f64::MIN, f64::max))
        .collect();
    let low: Vec<f64> = tapes
        .iter()
        .map(|prices| prices.iter().copied().fold(f64::MAX, f64::min))
        .collect();
    let entries = vec![0; n];
    let exits = vec![false; n];
    let strength = vec![0.0; n];
    let none = |_: &str| None;
    let inputs = CandleInputs {
        time_us: &time,
        open: Some(&open),
        high: Some(&high),
        low: Some(&low),
        close: None,
        signals: SignalColumns {
            entry: &entries,
            exit_long: &exits,
            exit_short: &exits,
            strength: &strength,
            bar_index: None,
        },
        volatility: None,
        tradable: None,
        columns: &none,
        protective: None,
        intrabar: Some(&tape),
    };
    let mut entry = entry;
    run_candle_with_callbacks(
        &inputs,
        &CandleConfig {
            initial_capital: 10_000.0,
            point_value: 1.0,
            costs: None,
            sizing: Sizing::FixedQuantity {
                quantity: 1.0,
                scale_by_strength: false,
            },
            holding_period_bars: None,
            exit_params: Default::default(),
            day_trade: None,
            force_close_at_end: false,
            latency_us: 0,
        },
        RuntimeCallbacks {
            strategy: Some(&mut *strategy as &mut StrategyFn<'_, CandleError>),
            entry: entry
                .as_mut()
                .map(|entry| &mut **entry as &mut StrategyFn<'_, CandleError>),
            entry_filled: None,
        },
    )
    .unwrap()
}

fn buy() -> BarSignals {
    BarSignals {
        entry: 1,
        strength: 1.0,
        ..BarSignals::default()
    }
}

fn candidate(side: Side, price: f64) -> EntryCandidate {
    EntryCandidate {
        side,
        price,
        levels: None,
    }
}

/// Bar 0 places `opening`; bar 1 trades 94 before 106 and offers `prices` as the long's exits.
fn close_long(opening: BarSignals, prices: Vec<f64>, close_next_open: bool) -> CandleRun {
    run(
        &[&[100.0], &[100.0, 94.0, 106.0, 103.0], &[102.0]],
        Box::new(move |bar, _| {
            Ok(match bar {
                0 => opening.clone(),
                1 => BarSignals {
                    exit_long: true,
                    exit_prices: prices.clone(),
                    close_next_open,
                    ..BarSignals::default()
                },
                _ => BarSignals::default(),
            })
        }),
        None,
    )
}

/// A buy stop at 104 that fills at bar 1's 106, after the 94 that every exit below it needs.
fn buy_stop() -> BarSignals {
    BarSignals {
        resting: Some(RestingOrder {
            kind: RestingKind::Stop,
            price: 104.0,
            expire_bars: None,
            expire_time_us: None,
        }),
        ..buy()
    }
}

#[test]
fn the_exit_price_the_tape_reaches_first_closes_the_trade() {
    for (prices, winner) in [(vec![95.0, 105.0], 0), (vec![105.0, 95.0], 1)] {
        let result = close_long(buy(), prices, false);
        assert_eq!(result.trades.exit_bar, vec![1]);
        // The stop at 95 is reached by the jump to 94, its fill.
        assert_eq!(result.trades.exit_price, vec![94.0]);
        assert_eq!(result.trades.exit_time_us, vec![11]);
        assert_eq!(result.trades.exit_candidate, vec![winner]);
        assert_eq!(result.trades.exit_reason, vec![ExitReason::Signal.code()]);
    }
}

#[test]
fn exit_prices_reached_on_the_same_tick_resolve_in_list_order() {
    let result = close_long(buy(), vec![96.0, 95.0], false);
    assert_eq!(result.trades.exit_price, vec![94.0]);
    assert_eq!(result.trades.exit_candidate, vec![0]);
}

#[test]
fn a_target_fills_at_its_price_once_traded_beyond_and_not_on_a_touch() {
    // The long's target at 106 is only touched, so the stop at 95 is the one that fills.
    let touched = close_long(buy(), vec![106.0, 95.0], false);
    assert_eq!(touched.trades.exit_candidate, vec![1]);
    // A target at 104 trades beyond at 106, after the stop's 94.
    let beyond = run(
        &[&[100.0], &[100.0, 105.0, 94.0], &[102.0]],
        Box::new(|bar, _| {
            Ok(match bar {
                0 => buy(),
                1 => BarSignals {
                    exit_long: true,
                    exit_prices: vec![95.0, 104.0],
                    ..BarSignals::default()
                },
                _ => BarSignals::default(),
            })
        }),
        None,
    );
    assert_eq!(beyond.trades.exit_price, vec![104.0]);
    assert_eq!(beyond.trades.exit_candidate, vec![1]);
}

#[test]
fn unreached_exit_prices_close_at_the_next_open_only_when_asked() {
    let kept = close_long(buy_stop(), vec![95.0], false);
    assert_eq!(kept.trades.entry_bar, vec![1]);
    assert_eq!(kept.trades.exit_bar, vec![-1]);

    let closed = close_long(buy_stop(), vec![95.0], true);
    assert_eq!(closed.trades.exit_bar, vec![2]);
    assert_eq!(closed.trades.exit_price, vec![102.0]);
    assert_eq!(closed.trades.exit_candidate, vec![-1]);
}

#[test]
fn the_earliest_touched_entry_candidate_fills_with_its_own_levels() {
    let result = run(
        &[&[100.0], &[100.0, 96.0, 106.0, 103.0]],
        Box::new(|bar, _| {
            Ok(if bar == 1 {
                BarSignals {
                    entry_candidates: Some(vec![
                        candidate(Side::Long, 105.0),
                        EntryCandidate {
                            levels: Some((110.0, 90.0)),
                            ..candidate(Side::Short, 96.0)
                        },
                    ]),
                    ..buy()
                }
            } else {
                BarSignals::default()
            })
        }),
        None,
    );
    assert_eq!(result.trades.side, vec![-1]);
    assert_eq!(result.trades.entry_price, vec![96.0]);
    assert_eq!(result.trades.entry_candidate, vec![1]);
    assert_eq!(result.trades.final_stop_price, vec![110.0]);
    assert_eq!(result.trades.final_target_price, vec![90.0]);
}

/// Splits decisions as a research host does: bar 0 buys at bar 1's open, bar 1 closes the long at
/// 95 (reached at its second price), then `offer` decides bar 1's entry.
fn exit_then_enter(
    tape: &'static [f64],
    offer: Vec<EntryCandidate>,
) -> (CandleRun, Vec<(usize, usize)>) {
    let seen = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let record = seen.clone();
    let result = run(
        &[&[100.0], tape],
        Box::new(|bar, _| {
            Ok(if bar == 1 {
                BarSignals {
                    exit_long: true,
                    exit_prices: vec![95.0],
                    ..BarSignals::default()
                }
            } else {
                BarSignals::default()
            })
        }),
        Some(Box::new(move |bar, positions| {
            record.borrow_mut().push((bar, positions.len()));
            Ok(match bar {
                0 => buy(),
                1 => BarSignals {
                    entry_candidates: Some(offer.clone()),
                    ..buy()
                },
                _ => BarSignals::default(),
            })
        })),
    );
    let seen = seen.borrow().clone();
    (result, seen)
}

#[test]
fn the_entry_callback_sees_the_positions_left_after_priced_closes() {
    let (result, seen) = exit_then_enter(
        &[100.0, 94.0, 104.0, 106.0],
        vec![candidate(Side::Long, 105.0), candidate(Side::Short, 103.0)],
    );
    assert_eq!(seen, vec![(0, 0), (1, 0)]);
    assert_eq!(result.trades.exit_price[0].to_bits(), 94.0_f64.to_bits());
    // After the exit at 94 the tape rises: the short at 103 is a limit traded beyond at 104, before
    // the long's stop at 105 fills at 106.
    assert_eq!(result.trades.side, vec![1, -1]);
    assert_eq!(result.trades.entry_price[1].to_bits(), 103.0_f64.to_bits());
    assert_eq!(result.trades.entry_candidate, vec![-1, 1]);
}

#[test]
fn entry_candidates_untouched_after_the_exit_are_all_rejected() {
    let (result, _) = exit_then_enter(
        &[100.0, 106.0, 94.0, 96.0],
        vec![candidate(Side::Long, 105.0), candidate(Side::Short, 100.0)],
    );
    assert_eq!(result.trades.entry_bar, vec![1]);
    assert_eq!(result.rejected.bar, vec![1, 1]);
    assert_eq!(result.rejected.fill_price, vec![105.0, 100.0]);
    assert_eq!(result.rejected.reason, vec![1, 1]);
}

#[test]
fn a_split_strategy_callback_may_not_decide_entries() {
    let tape = Tape(vec![vec![100.0]]);
    let time = [0];
    let price = [100.0];
    let entries = [0];
    let exits = [false];
    let strength = [0.0];
    let none = |_: &str| None;
    let inputs = CandleInputs {
        time_us: &time,
        open: Some(&price),
        high: Some(&price),
        low: Some(&price),
        close: None,
        signals: SignalColumns {
            entry: &entries,
            exit_long: &exits,
            exit_short: &exits,
            strength: &strength,
            bar_index: None,
        },
        volatility: None,
        tradable: None,
        columns: &none,
        protective: None,
        intrabar: Some(&tape),
    };
    let mut strategy = |_: usize, _: &[PositionSnapshot]| Ok::<_, CandleError>(buy());
    let mut entry = |_: usize, _: &[PositionSnapshot]| Ok::<_, CandleError>(BarSignals::default());
    let result = run_candle_with_callbacks(
        &inputs,
        &CandleConfig {
            initial_capital: 10_000.0,
            point_value: 1.0,
            costs: None,
            sizing: Sizing::FixedQuantity {
                quantity: 1.0,
                scale_by_strength: false,
            },
            holding_period_bars: None,
            exit_params: Default::default(),
            day_trade: None,
            force_close_at_end: false,
            latency_us: 0,
        },
        RuntimeCallbacks {
            strategy: Some(&mut strategy),
            entry: Some(&mut entry),
            entry_filled: None,
        },
    );
    assert!(matches!(
        result,
        Err(CandleError::InvalidSignal {
            column: "entry",
            ..
        })
    ));
}
