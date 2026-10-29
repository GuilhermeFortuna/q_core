#![forbid(unsafe_code)]

//! Hand-computed cases for resting entry orders and stop and target amendments. Each series is
//! small enough to check by hand. The tape source records the bars the kernel asked for, so a
//! bar the range cannot reach must not appear in it.

use std::cell::RefCell;
use std::collections::BTreeMap;

use q_engine::{
    run_candle_with_callback, BarSignals, CandleConfig, CandleError, CandleInputs, CandleRun,
    DayTradeWindow, EntryCandidate, ExitParams, ExitReason, IntrabarPrices, IntrabarSource,
    RestingKind, RestingOrder, Side, SignalColumns, Sizing,
};

#[test]
fn amendments_require_an_intrabar_source() {
    let series = Series::hourly(&[(100.0, 100.0, 100.0); 3]);
    for signals in [
        BarSignals {
            amend_stop: Some(95.0),
            ..BarSignals::default()
        },
        BarSignals {
            amend_target: Some(105.0),
            ..BarSignals::default()
        },
    ] {
        let error = series
            .run(&[(0, enter(1)), (1, signals)], None, &Series::config(None))
            .expect_err("amendments cannot be enforced without trade prices");
        assert!(matches!(error, CandleError::InvalidConfig { .. }));
    }
}

const SECOND: i64 = 1_000_000;
const MINUTE: i64 = 60 * SECOND;
const HOUR: i64 = 60 * MINUTE;
const US_PER_DAY: i64 = 24 * HOUR;

/// Wall-clock time of `minute` past midnight on `day`.
fn at(day: i64, minute: i64) -> i64 {
    day * US_PER_DAY + minute * MINUTE
}

/// Wall-clock time of hourly bar `bar` in a series that starts at 10:00 on day 0.
fn bar_time(bar: usize) -> i64 {
    at(0, 10 * 60 + 60 * bar as i64)
}

fn bits(values: &[f64]) -> Vec<u64> {
    values.iter().map(|value| value.to_bits()).collect()
}

/// Serves the listed bars' prices, one second apart from the bar's first bar time, and records each
/// call. A bar that is not listed is an error, so an unexpected call fails the run.
#[derive(Default)]
struct Tape {
    bars: BTreeMap<usize, IntrabarPrices>,
    calls: RefCell<Vec<usize>>,
}

impl Tape {
    /// Prices for the listed bars, timed from each bar's own time stamp `times[bar]`.
    fn new(times: &[i64], bars: &[(usize, &[f64])]) -> Self {
        let bars = bars
            .iter()
            .map(|&(bar, prices)| {
                let time_us = (0..prices.len())
                    .map(|i| times[bar] + i as i64 * SECOND)
                    .collect();
                let prices = IntrabarPrices {
                    time_us,
                    price: prices.to_vec(),
                };
                (bar, prices)
            })
            .collect();
        Self {
            bars,
            calls: RefCell::new(Vec::new()),
        }
    }
}

impl IntrabarSource for Tape {
    fn prices(&self, bar: usize) -> Result<IntrabarPrices, String> {
        self.calls.borrow_mut().push(bar);
        self.bars
            .get(&bar)
            .cloned()
            .ok_or_else(|| format!("no tape for bar {bar}"))
    }
}

/// A series of bars with their open, high and low, and the time stamp of each.
struct Series {
    time_us: Vec<i64>,
    open: Vec<f64>,
    high: Vec<f64>,
    low: Vec<f64>,
}

impl Series {
    /// Hourly bars from 10:00 on day 0, one `(open, high, low)` each.
    fn hourly(bars: &[(f64, f64, f64)]) -> Self {
        Self::timed(&(0..bars.len()).map(bar_time).collect::<Vec<_>>(), bars)
    }

    fn timed(time_us: &[i64], bars: &[(f64, f64, f64)]) -> Self {
        Self {
            time_us: time_us.to_vec(),
            open: bars.iter().map(|bar| bar.0).collect(),
            high: bars.iter().map(|bar| bar.1).collect(),
            low: bars.iter().map(|bar| bar.2).collect(),
        }
    }

    fn config(day_trade: Option<DayTradeWindow>) -> CandleConfig {
        CandleConfig {
            initial_capital: 10_000.0,
            point_value: 1.0,
            costs: None,
            sizing: Sizing::FixedQuantity {
                quantity: 1.0,
                scale_by_strength: false,
            },
            holding_period_bars: None,
            exit_params: ExitParams::default(),
            day_trade,
            force_close_at_end: false,
            latency_us: 0,
        }
    }

    /// Runs the series with `decisions` as the strategy's per-bar signals; other bars decide nothing.
    fn run(
        &self,
        decisions: &[(usize, BarSignals)],
        intrabar: Option<&dyn IntrabarSource>,
        config: &CandleConfig,
    ) -> Result<CandleRun, CandleError> {
        let n = self.time_us.len();
        let entry = vec![0_i8; n];
        let strength = vec![0.0; n];
        let no_exit = vec![false; n];
        let no_columns = |_: &str| None;
        let inputs = CandleInputs {
            time_us: &self.time_us,
            open: Some(&self.open),
            high: Some(&self.high),
            low: Some(&self.low),
            close: None,
            signals: SignalColumns {
                entry: &entry,
                exit_long: &no_exit,
                exit_short: &no_exit,
                strength: &strength,
                bar_index: None,
            },
            volatility: None,
            tradable: None,
            columns: &no_columns,
            protective: None,
            intrabar,
        };
        let mut by_bar = vec![BarSignals::default(); n];
        for (bar, signals) in decisions {
            by_bar[*bar] = signals.clone();
        }
        run_candle_with_callback(&inputs, config, |bar, _| {
            Ok::<_, CandleError>(by_bar[bar].clone())
        })
    }
}

/// A market entry decided on the bar it is returned for.
fn enter(side: i8) -> BarSignals {
    BarSignals {
        entry: side,
        strength: 1.0,
        ..BarSignals::default()
    }
}

/// A resting entry order placed on the bar it is returned for.
fn place(side: i8, kind: RestingKind, price: f64, expire_bars: Option<usize>) -> BarSignals {
    BarSignals {
        entry: side,
        strength: 1.0,
        resting: Some(RestingOrder {
            kind,
            price,
            expire_bars,
            expire_time_us: None,
        }),
        ..BarSignals::default()
    }
}

/// Like `place`, with the stop and target the filled trade carries.
fn place_with_levels(side: i8, kind: RestingKind, price: f64, levels: (f64, f64)) -> BarSignals {
    BarSignals {
        levels: Some(levels),
        ..place(side, kind, price, None)
    }
}

const STOP: RestingKind = RestingKind::Stop;
const LIMIT: RestingKind = RestingKind::Limit;

#[test]
fn a_buy_stop_fills_at_the_first_trade_at_or_beyond_its_price_even_after_a_gap() {
    // Bar 1 opens at 102, through the stop at 101, so the fill is the gap price.
    let series = Series::hourly(&[(100.0, 100.0, 100.0), (102.0, 103.0, 102.0)]);
    let tape = Tape::new(&[bar_time(0), bar_time(1)], &[(1, &[102.0, 103.0])]);
    let run = series
        .run(
            &[(0, place(1, STOP, 101.0, None))],
            Some(&tape),
            &Series::config(None),
        )
        .unwrap();

    assert_eq!(run.trades.entry_bar, vec![1]);
    assert_eq!(bits(&run.trades.entry_price), bits(&[102.0]));
    assert_eq!(run.trades.entry_kind, vec![2]);
    assert_eq!(run.trades.entry_time_us, vec![bar_time(1)]);
}

#[test]
fn a_buy_stop_fills_at_the_crossing_price_inside_the_bar() {
    let series = Series::hourly(&[(100.0, 100.0, 100.0), (100.0, 101.5, 100.0)]);
    let tape = Tape::new(&[bar_time(0), bar_time(1)], &[(1, &[100.0, 100.5, 101.5])]);
    let run = series
        .run(
            &[(0, place(1, STOP, 101.0, None))],
            Some(&tape),
            &Series::config(None),
        )
        .unwrap();

    assert_eq!(bits(&run.trades.entry_price), bits(&[101.5]));
    assert_eq!(run.trades.entry_time_us, vec![bar_time(1) + 2 * SECOND]);
}

#[test]
fn a_buy_limit_fills_at_its_price_once_a_trade_prints_strictly_below_it() {
    // The 99.0 prints equal the limit and do not fill it; 98.5 does, at 99.
    let series = Series::hourly(&[(100.0, 100.0, 100.0), (100.0, 100.0, 98.5)]);
    let tape = Tape::new(&[bar_time(0), bar_time(1)], &[(1, &[99.0, 99.0, 98.5])]);
    let run = series
        .run(
            &[(0, place(1, LIMIT, 99.0, None))],
            Some(&tape),
            &Series::config(None),
        )
        .unwrap();

    assert_eq!(bits(&run.trades.entry_price), bits(&[99.0]));
    assert_eq!(run.trades.entry_kind, vec![3]);
    assert_eq!(run.trades.entry_time_us, vec![bar_time(1) + 2 * SECOND]);
}

#[test]
fn a_buy_limit_is_not_read_when_the_range_never_goes_below_it() {
    let series = Series::hourly(&[(100.0, 100.0, 100.0), (100.0, 100.0, 99.0)]);
    let tape = Tape::new(&[bar_time(0), bar_time(1)], &[]);
    let run = series
        .run(
            &[(0, place(1, LIMIT, 99.0, None))],
            Some(&tape),
            &Series::config(None),
        )
        .unwrap();

    assert!(run.trades.entry_bar.is_empty());
    assert!(tape.calls.borrow().is_empty());
}

#[test]
fn sell_stops_and_limits_mirror_the_buy_rules() {
    // A sell stop at 99 fills at the first print at or below it.
    let series = Series::hourly(&[(100.0, 100.0, 100.0), (100.0, 100.0, 98.5)]);
    let tape = Tape::new(&[bar_time(0), bar_time(1)], &[(1, &[100.0, 98.5])]);
    let run = series
        .run(
            &[(0, place(-1, STOP, 99.0, None))],
            Some(&tape),
            &Series::config(None),
        )
        .unwrap();
    assert_eq!(run.trades.side, vec![-1]);
    assert_eq!(bits(&run.trades.entry_price), bits(&[98.5]));
    assert_eq!(run.trades.entry_kind, vec![2]);

    // A sell limit at 101 fills at 101 once a print is strictly above it.
    let series = Series::hourly(&[(100.0, 100.0, 100.0), (100.0, 102.0, 100.0)]);
    let tape = Tape::new(&[bar_time(0), bar_time(1)], &[(1, &[100.0, 101.0, 101.5])]);
    let run = series
        .run(
            &[(0, place(-1, LIMIT, 101.0, None))],
            Some(&tape),
            &Series::config(None),
        )
        .unwrap();
    assert_eq!(run.trades.side, vec![-1]);
    assert_eq!(bits(&run.trades.entry_price), bits(&[101.0]));
    assert_eq!(run.trades.entry_time_us, vec![bar_time(1) + 2 * SECOND]);
}

#[test]
fn an_order_with_expire_bars_fills_on_the_bar_it_reaches_and_then_lapses() {
    // Bars 1 and 2 stay below the stop at 101; bar 3 reaches it, one bar past the expiry of 2.
    let series = Series::hourly(&[
        (100.0, 100.0, 100.0),
        (100.0, 100.5, 100.0),
        (100.0, 100.5, 100.0),
        (100.0, 101.5, 100.0),
    ]);
    let tape = Tape::new(
        &[bar_time(0), bar_time(1), bar_time(2), bar_time(3)],
        &[(3, &[101.5])],
    );
    let run = series
        .run(
            &[(0, place(1, STOP, 101.0, Some(2)))],
            Some(&tape),
            &Series::config(None),
        )
        .unwrap();
    assert!(run.trades.entry_bar.is_empty());
    assert!(
        tape.calls.borrow().is_empty(),
        "a lapsed order makes no source call"
    );

    // Without an expiry the same order fills on bar 3.
    let tape = Tape::new(
        &[bar_time(0), bar_time(1), bar_time(2), bar_time(3)],
        &[(3, &[101.5])],
    );
    let run = series
        .run(
            &[(0, place(1, STOP, 101.0, None))],
            Some(&tape),
            &Series::config(None),
        )
        .unwrap();
    assert_eq!(run.trades.entry_bar, vec![3]);
    assert_eq!(*tape.calls.borrow(), vec![3]);

    // With expire_bars = 2 the order fills on bar 2, and only once.
    let series = Series::hourly(&[
        (100.0, 100.0, 100.0),
        (100.0, 100.5, 100.0),
        (100.0, 101.5, 100.0),
        (100.0, 101.5, 100.0),
    ]);
    let tape = Tape::new(
        &[bar_time(0), bar_time(1), bar_time(2), bar_time(3)],
        &[(2, &[101.5]), (3, &[101.5])],
    );
    let run = series
        .run(
            &[(0, place(1, STOP, 101.0, Some(2)))],
            Some(&tape),
            &Series::config(None),
        )
        .unwrap();
    assert_eq!(run.trades.entry_bar, vec![2]);
}

#[test]
fn day_trade_mode_lapses_a_working_order_after_the_last_bar_of_its_day() {
    // Day 0 has two bars and day 1 two more. The order placed at 10:00 is still working on 11:00,
    // the last bar of day 0; a reach on day 1's first bar must not fill it.
    let times = [at(0, 600), at(0, 660), at(1, 600), at(1, 660)];
    let series = Series::timed(
        &times,
        &[
            (100.0, 100.0, 100.0),
            (100.0, 100.0, 100.0),
            (100.0, 102.0, 100.0),
            (100.0, 102.0, 100.0),
        ],
    );
    let window = DayTradeWindow {
        entry_start_us: at(0, 600),
        entry_end_us: at(0, 630),
        force_close_us: at(0, 900),
    };
    let tape = Tape::new(&times, &[(2, &[101.5])]);
    let run = series
        .run(
            &[(0, place(1, STOP, 101.0, None))],
            Some(&tape),
            &Series::config(Some(window)),
        )
        .unwrap();
    assert!(run.trades.entry_bar.is_empty());
    assert!(tape.calls.borrow().is_empty());

    // The same order fills on day 0's last bar when that bar reaches it.
    let series = Series::timed(
        &times,
        &[
            (100.0, 100.0, 100.0),
            (100.0, 101.5, 100.0),
            (100.0, 102.0, 100.0),
            (100.0, 102.0, 100.0),
        ],
    );
    let tape = Tape::new(&times, &[(1, &[101.5])]);
    let run = series
        .run(
            &[(0, place(1, STOP, 101.0, None))],
            Some(&tape),
            &Series::config(Some(window)),
        )
        .unwrap();
    assert_eq!(run.trades.entry_bar, vec![1]);
}

#[test]
fn an_order_decided_outside_the_entry_window_places_nothing() {
    // Entries are allowed until 10:30. The order decided at 11:00 is gated, so the 12:00 reach
    // does not fill it.
    let times = [at(0, 600), at(0, 660), at(0, 720), at(1, 600)];
    let series = Series::timed(
        &times,
        &[
            (100.0, 100.0, 100.0),
            (100.0, 100.0, 100.0),
            (100.0, 102.0, 100.0),
            (100.0, 100.0, 100.0),
        ],
    );
    let window = DayTradeWindow {
        entry_start_us: at(0, 600),
        entry_end_us: at(0, 630),
        force_close_us: at(0, 900),
    };
    let tape = Tape::new(&times, &[(2, &[101.5])]);
    let run = series
        .run(
            &[(1, place(1, STOP, 101.0, None))],
            Some(&tape),
            &Series::config(Some(window)),
        )
        .unwrap();
    assert!(run.trades.entry_bar.is_empty());
    assert!(tape.calls.borrow().is_empty());
}

#[test]
fn a_new_entry_replaces_the_working_order_and_a_cancel_removes_it() {
    let series = Series::hourly(&[
        (100.0, 100.0, 100.0),
        (100.0, 100.0, 100.0),
        (100.0, 102.0, 100.0),
    ]);

    // The market entry decided on bar 1 replaces the stop order; it opens at bar 2's open of 100.
    let tape = Tape::new(
        &[bar_time(0), bar_time(1), bar_time(2)],
        &[(2, &[100.0, 101.5, 102.0])],
    );
    let run = series
        .run(
            &[(0, place(1, STOP, 101.0, None)), (1, enter(1))],
            Some(&tape),
            &Series::config(None),
        )
        .unwrap();
    assert_eq!(run.trades.entry_kind, vec![0]);
    assert_eq!(bits(&run.trades.entry_price), bits(&[100.0]));
    assert!(tape.calls.borrow().is_empty());

    // A cancel removes the order, which then never fills.
    let tape = Tape::new(&[bar_time(0), bar_time(1), bar_time(2)], &[(2, &[101.5])]);
    let cancel = BarSignals {
        cancel_entry: true,
        ..BarSignals::default()
    };
    let run = series
        .run(
            &[(0, place(1, STOP, 101.0, None)), (1, cancel)],
            Some(&tape),
            &Series::config(None),
        )
        .unwrap();
    assert!(run.trades.entry_bar.is_empty());
    assert!(tape.calls.borrow().is_empty());
}

#[test]
fn a_trade_opened_by_a_stop_order_can_stop_out_on_the_same_bar() {
    // The order at 101 fills on the second print. The stop at 100 is reached by the fourth.
    let series = Series::hourly(&[(100.0, 100.0, 100.0), (100.5, 101.0, 100.0)]);
    let tape = Tape::new(
        &[bar_time(0), bar_time(1)],
        &[(1, &[100.5, 101.0, 100.5, 100.0])],
    );
    let run = series
        .run(
            &[(0, place_with_levels(1, STOP, 101.0, (100.0, f64::NAN)))],
            Some(&tape),
            &Series::config(None),
        )
        .unwrap();

    assert_eq!(run.trades.entry_bar, vec![1]);
    assert_eq!(run.trades.entry_time_us, vec![bar_time(1) + SECOND]);
    assert_eq!(run.trades.exit_bar, vec![1]);
    assert_eq!(bits(&run.trades.exit_price), bits(&[100.0]));
    assert_eq!(run.trades.exit_reason, vec![ExitReason::StopLoss.code()]);
    assert_eq!(run.trades.exit_time_us, vec![bar_time(1) + 3 * SECOND]);
}

#[test]
fn priced_entry_after_resting_fill_and_same_bar_stop_requires_a_later_touch() {
    let series = Series::hourly(&[(100.0, 102.0, 99.0), (100.0, 105.0, 95.0)]);
    let tape = Tape::new(&series.time_us, &[(1, &[101.0, 99.0])]);
    let result = series
        .run(
            &[
                (
                    0,
                    BarSignals {
                        entry: 1,
                        strength: 1.0,
                        resting: Some(RestingOrder {
                            kind: RestingKind::Stop,
                            price: 101.0,
                            expire_bars: None,
                            expire_time_us: None,
                        }),
                        levels: Some((99.0, f64::NAN)),
                        ..BarSignals::default()
                    },
                ),
                (
                    1,
                    BarSignals {
                        entry: 1,
                        strength: 1.0,
                        entry_candidates: Some(vec![EntryCandidate {
                            side: Side::Long,
                            price: 100.0,
                            levels: None,
                        }]),
                        ..BarSignals::default()
                    },
                ),
            ],
            Some(&tape),
            &Series::config(None),
        )
        .unwrap();

    assert_eq!(result.trades.entry_kind, vec![2]);
    assert_eq!(bits(&result.trades.exit_price[..1]), bits(&[99.0]));
    assert_eq!(result.trades.exit_time_us, vec![series.time_us[1] + SECOND]);
    assert_eq!(result.rejected.bar, vec![1]);
    assert_eq!(result.rejected.reason, vec![1]);
}

/// A long opened at 100 on bar 1 with a stop at 95 and a long-run series of bars 2 and 3.
fn amended_long(bar_two: (f64, f64, f64), bar_three: (f64, f64, f64)) -> (CandleRun, Tape) {
    let series = Series::hourly(&[
        (100.0, 100.0, 100.0),
        (100.0, 100.0, 100.0),
        bar_two,
        bar_three,
    ]);
    let tape = Tape::new(
        &[bar_time(0), bar_time(1), bar_time(2), bar_time(3)],
        &[(3, &[bar_three.0, 98.0])],
    );
    let amend = |stop: f64| BarSignals {
        amend_stop: Some(stop),
        ..BarSignals::default()
    };
    let run = series
        .run(
            &[
                (
                    0,
                    BarSignals {
                        levels: Some((95.0, f64::NAN)),
                        ..enter(1)
                    },
                ),
                (2, amend(98.0)),
            ],
            Some(&tape),
            &Series::config(None),
        )
        .unwrap();
    (run, tape)
}

#[test]
fn an_amended_stop_is_invisible_on_its_deciding_bar_and_triggers_on_the_next() {
    // Bar 2 trades 97.5: below the new stop of 98 but above the old stop of 95, and the amendment
    // is not in force on its deciding bar, so bar 2 makes no source call.
    let (run, tape) = amended_long((100.0, 100.5, 97.5), (98.5, 99.0, 97.5));

    assert_eq!(*tape.calls.borrow(), vec![3]);
    assert_eq!(run.trades.exit_bar, vec![3]);
    assert_eq!(bits(&run.trades.exit_price), bits(&[98.0]));
    assert_eq!(run.trades.exit_reason, vec![ExitReason::StopLoss.code()]);
    assert_eq!(bits(&run.trades.final_stop_price), bits(&[98.0]));
}

#[test]
fn an_amended_stop_above_the_next_open_fills_at_that_open() {
    // The amended stop of 98 is already beyond bar 3's open of 97; the fill is that open.
    let series = Series::hourly(&[
        (100.0, 100.0, 100.0),
        (100.0, 100.0, 100.0),
        (100.0, 100.5, 99.5),
        (97.0, 97.5, 96.5),
    ]);
    let tape = Tape::new(
        &[bar_time(0), bar_time(1), bar_time(2), bar_time(3)],
        &[(3, &[97.0, 96.5])],
    );
    let run = series
        .run(
            &[
                (
                    0,
                    BarSignals {
                        levels: Some((95.0, f64::NAN)),
                        ..enter(1)
                    },
                ),
                (
                    2,
                    BarSignals {
                        amend_stop: Some(98.0),
                        ..BarSignals::default()
                    },
                ),
            ],
            Some(&tape),
            &Series::config(None),
        )
        .unwrap();

    assert_eq!(run.trades.exit_bar, vec![3]);
    assert_eq!(bits(&run.trades.exit_price), bits(&[97.0]));
    assert_eq!(run.trades.exit_time_us, vec![bar_time(3)]);
}

#[test]
fn a_protective_exit_and_a_reversing_resting_entry_on_one_price_close_then_open() {
    // A long opened at 100 with its stop at 95. Bar 1 places a sell stop at 95, which bar 2 reaches
    // with the same print as the long's stop.
    let series = Series::hourly(&[
        (100.0, 100.0, 100.0),
        (100.0, 100.0, 100.0),
        (96.0, 96.0, 94.0),
    ]);
    let tape = Tape::new(
        &[bar_time(0), bar_time(1), bar_time(2)],
        &[(2, &[95.0, 94.0])],
    );
    let run = series
        .run(
            &[
                (
                    0,
                    BarSignals {
                        levels: Some((95.0, f64::NAN)),
                        ..enter(1)
                    },
                ),
                (1, place(-1, STOP, 95.0, None)),
            ],
            Some(&tape),
            &Series::config(None),
        )
        .unwrap();

    assert_eq!(run.trades.side, vec![1, -1]);
    assert_eq!(run.trades.exit_bar, vec![2, -1]);
    assert_eq!(bits(&run.trades.exit_price), bits(&[95.0, f64::NAN]));
    assert_eq!(
        run.trades.exit_reason,
        vec![ExitReason::StopLoss.code(), -1]
    );
    assert_eq!(bits(&run.trades.entry_price), bits(&[100.0, 95.0]));
    assert_eq!(run.trades.entry_kind, vec![0, 2]);
}

#[test]
fn a_capped_order_is_skipped_and_not_reported_even_when_wrong_side() {
    // The long from bar 0 holds the single unit the cap allows. The order at 101 fills on bar 2
    // with a stop of 102, which is wrong-side, but the cap leaves it nothing, so it is not rejected.
    let series = Series::hourly(&[
        (100.0, 100.0, 100.0),
        (100.0, 100.0, 100.0),
        (100.0, 101.5, 100.0),
    ]);
    let tape = Tape::new(&[bar_time(0), bar_time(1), bar_time(2)], &[(2, &[101.5])]);
    let run = series
        .run(
            &[
                (0, enter(1)),
                (1, place_with_levels(1, STOP, 101.0, (102.0, f64::NAN))),
            ],
            Some(&tape),
            &Series::config(None),
        )
        .unwrap();

    assert_eq!(run.trades.entry_bar, vec![1]);
    assert!(run.rejected.bar.is_empty());
}

#[test]
fn an_uncapped_wrong_side_order_is_rejected_at_its_fill_price() {
    let series = Series::hourly(&[(100.0, 100.0, 100.0), (100.0, 102.0, 100.0)]);
    let tape = Tape::new(&[bar_time(0), bar_time(1)], &[(1, &[101.5])]);
    let run = series
        .run(
            &[(0, place_with_levels(1, STOP, 101.0, (102.0, f64::NAN)))],
            Some(&tape),
            &Series::config(None),
        )
        .unwrap();

    assert!(run.trades.entry_bar.is_empty());
    assert_eq!(run.rejected.bar, vec![1]);
    assert_eq!(run.rejected.side, vec![1]);
    assert_eq!(bits(&run.rejected.fill_price), bits(&[101.5]));
}

#[test]
fn resting_orders_need_intrabar_prices() {
    let series = Series::hourly(&[(100.0, 100.0, 100.0), (100.0, 100.0, 100.0)]);
    let result = series.run(
        &[(0, place(1, STOP, 101.0, None))],
        None,
        &Series::config(None),
    );
    assert!(matches!(
        result,
        Err(CandleError::InvalidConfig {
            field: "resting_order",
            ..
        })
    ));
}

#[test]
fn invalid_resting_and_amendment_fields_name_their_column() {
    let series = Series::hourly(&[(100.0, 100.0, 100.0), (100.0, 100.0, 100.0)]);
    let tape = Tape::new(&[bar_time(0), bar_time(1)], &[]);
    let cases = [
        (
            BarSignals {
                resting: Some(RestingOrder {
                    kind: STOP,
                    price: 101.0,
                    expire_bars: None,
                    expire_time_us: None,
                }),
                ..BarSignals::default()
            },
            "entry",
        ),
        (place(1, STOP, -1.0, None), "entry_price"),
        (place(1, STOP, 101.0, Some(0)), "expire_bars"),
        (
            BarSignals {
                amend_stop: Some(f64::NAN),
                ..BarSignals::default()
            },
            "amend_stop",
        ),
        (
            BarSignals {
                amend_target: Some(0.0),
                ..BarSignals::default()
            },
            "amend_target",
        ),
    ];
    for (signals, column) in cases {
        let result = series.run(&[(0, signals)], Some(&tape), &Series::config(None));
        assert!(
            matches!(result, Err(CandleError::InvalidSignal { column: named, bar: 0, .. }) if named == column),
            "expected an error naming {column}"
        );
    }
}
