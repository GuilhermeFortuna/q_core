#![forbid(unsafe_code)]

//! Hand-computed cases for intrabar stop and target fills. Each series is small enough to check
//! by hand; the tape source records the bars the kernel asked for.

use std::cell::RefCell;
use std::collections::BTreeMap;

use q_engine::{
    run_candle, CandleConfig, CandleError, CandleInputs, CandleRun, DayTradeWindow, ExitParams,
    ExitReason, IntrabarPrices, IntrabarSource, ProtectiveColumns, SignalColumns, Sizing,
};

const HOUR: i64 = 3_600_000_000;
const SECOND: i64 = 1_000_000;

/// Wall-clock time of bar `bar` in the hourly series that starts at 10:00.
fn bar_time(bar: usize) -> i64 {
    (10 + bar as i64) * HOUR
}

fn bits(values: &[f64]) -> Vec<u64> {
    values.iter().map(|value| value.to_bits()).collect()
}

/// Serves the listed bars' prices, one second apart from the bar's open, and records each call.
/// A bar that is not listed is an error, so an unexpected call fails the run.
#[derive(Default)]
struct Tape {
    bars: BTreeMap<usize, IntrabarPrices>,
    calls: RefCell<Vec<usize>>,
}

impl Tape {
    fn new(bars: &[(usize, &[f64])]) -> Self {
        let bars = bars
            .iter()
            .map(|&(bar, prices)| {
                let time_us = (0..prices.len())
                    .map(|i| bar_time(bar) + i as i64 * SECOND)
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

/// A source that always fails, to check the error names the bar.
struct Failing;

impl IntrabarSource for Failing {
    fn prices(&self, _bar: usize) -> Result<IntrabarPrices, String> {
        Err("feed down".to_string())
    }
}

/// Hand-built series with per-bar protective levels; `NaN` means no level.
struct Scenario {
    time_us: Vec<i64>,
    open: Vec<f64>,
    high: Vec<f64>,
    low: Vec<f64>,
    close: Option<Vec<f64>>,
    entry: Vec<i8>,
    strength: Vec<f64>,
    stop: Vec<f64>,
    target: Vec<f64>,
}

impl Scenario {
    /// One hourly bar per `(open, high, low)` from 10:00, with no signals and no levels.
    fn hourly(bars: &[(f64, f64, f64)]) -> Self {
        let n = bars.len();
        Self {
            time_us: (0..n).map(bar_time).collect(),
            open: bars.iter().map(|bar| bar.0).collect(),
            high: bars.iter().map(|bar| bar.1).collect(),
            low: bars.iter().map(|bar| bar.2).collect(),
            close: None,
            entry: vec![0; n],
            strength: vec![0.0; n],
            stop: vec![f64::NAN; n],
            target: vec![f64::NAN; n],
        }
    }

    /// Queues an entry on `bar`; it fills at the next bar's open.
    fn enter(mut self, bar: usize, side: i8) -> Self {
        self.entry[bar] = side;
        self.strength[bar] = 1.0;
        self
    }

    /// Sets the levels an entry queued on `bar` carries.
    fn levels(mut self, bar: usize, stop: f64, target: f64) -> Self {
        self.stop[bar] = stop;
        self.target[bar] = target;
        self
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
        }
    }

    fn run_with(
        &self,
        intrabar: Option<&dyn IntrabarSource>,
        config: &CandleConfig,
    ) -> Result<CandleRun, CandleError> {
        let n = self.time_us.len();
        let no_exit = vec![false; n];
        let no_columns = |_: &str| None;
        let inputs = CandleInputs {
            time_us: &self.time_us,
            open: Some(&self.open),
            high: Some(&self.high),
            low: Some(&self.low),
            close: self.close.as_deref(),
            signals: SignalColumns {
                entry: &self.entry,
                exit_long: &no_exit,
                exit_short: &no_exit,
                strength: &self.strength,
                bar_index: None,
            },
            volatility: None,
            tradable: None,
            columns: &no_columns,
            protective: Some(ProtectiveColumns {
                stop_price: &self.stop,
                target_price: &self.target,
                entry_price: None,
            }),
            intrabar,
        };
        run_candle(&inputs, config)
    }

    fn run(&self, tape: &Tape) -> CandleRun {
        self.run_with(Some(tape), &Self::config(None))
            .expect("hand-built series is valid")
    }
}

/// Long entry queued on bar 0 with stop 95 and target 110; it fills at bar 1's open of 100.
/// Bar 2 carries `range` and `prices`.
fn long_into_bar_two(range: (f64, f64, f64), prices: &[f64]) -> (CandleRun, Tape) {
    let scenario = Scenario::hourly(&[(100.0, 100.0, 100.0), (100.0, 100.0, 100.0), range])
        .enter(0, 1)
        .levels(0, 95.0, 110.0);
    let tape = Tape::new(&[(2, prices)]);
    (scenario.run(&tape), tape)
}

#[test]
fn a_long_stop_fills_at_the_first_price_at_or_below_the_level() {
    let (run, tape) = long_into_bar_two((100.0, 101.0, 94.0), &[101.0, 96.0, 95.0, 94.0]);

    assert_eq!(run.trades.entry_bar, vec![1]);
    assert_eq!(run.trades.exit_bar, vec![2]);
    assert_eq!(bits(&run.trades.exit_price), bits(&[95.0]));
    assert_eq!(run.trades.exit_reason, vec![ExitReason::StopLoss.code()]);
    assert_eq!(run.trades.exit_time_us, vec![bar_time(2) + 2 * SECOND]);
    assert_eq!(*tape.calls.borrow(), vec![2]);
}

#[test]
fn a_short_stop_fills_at_the_first_price_at_or_above_the_level() {
    let scenario = Scenario::hourly(&[
        (100.0, 100.0, 100.0),
        (100.0, 100.0, 100.0),
        (100.0, 106.0, 99.0),
    ])
    .enter(0, -1)
    .levels(0, 105.0, 90.0);
    let tape = Tape::new(&[(2, &[99.0, 104.0, 105.0, 106.0])]);

    let run = scenario.run(&tape);

    assert_eq!(run.trades.side, vec![-1]);
    assert_eq!(run.trades.exit_bar, vec![2]);
    assert_eq!(bits(&run.trades.exit_price), bits(&[105.0]));
    assert_eq!(run.trades.exit_reason, vec![ExitReason::StopLoss.code()]);
    assert_eq!(*tape.calls.borrow(), vec![2]);
}

#[test]
fn a_bar_holding_both_levels_resolves_by_price_order() {
    let (run, _) = long_into_bar_two((100.0, 111.0, 94.0), &[100.0, 111.0, 94.0]);
    assert_eq!(bits(&run.trades.exit_price), bits(&[110.0]));
    assert_eq!(run.trades.exit_reason, vec![ExitReason::TakeProfit.code()]);

    let (run, _) = long_into_bar_two((100.0, 111.0, 94.0), &[100.0, 94.0, 111.0]);
    assert_eq!(bits(&run.trades.exit_price), bits(&[94.0]));
    assert_eq!(run.trades.exit_reason, vec![ExitReason::StopLoss.code()]);
}

#[test]
fn a_price_equal_to_the_target_does_not_fill() {
    let (run, _) = long_into_bar_two((100.0, 111.0, 100.0), &[110.0, 111.0]);

    assert_eq!(bits(&run.trades.exit_price), bits(&[110.0]));
    assert_eq!(
        run.trades.exit_time_us,
        vec![bar_time(2) + SECOND],
        "the fill is at the price strictly beyond the target, not the one equal to it"
    );
    assert_eq!(run.trades.exit_reason, vec![ExitReason::TakeProfit.code()]);
}

#[test]
fn a_bar_that_opens_beyond_the_stop_fills_at_its_open() {
    let (run, _) = long_into_bar_two((93.0, 100.0, 92.0), &[93.0, 94.0]);

    assert_eq!(bits(&run.trades.exit_price), bits(&[93.0]));
    assert_eq!(run.trades.exit_time_us, vec![bar_time(2)]);
}

#[test]
fn on_the_entry_bar_the_first_price_is_skipped() {
    // The entry fills at bar 1's open of 100; the stop at 99 is reached by the third price.
    let scenario = Scenario::hourly(&[(100.0, 100.0, 100.0), (100.0, 100.0, 99.0)])
        .enter(0, 1)
        .levels(0, 99.0, f64::NAN);
    let tape = Tape::new(&[(1, &[100.0, 99.5, 99.0])]);

    let run = scenario.run(&tape);

    assert_eq!(run.trades.entry_bar, vec![1]);
    assert_eq!(run.trades.exit_bar, vec![1]);
    assert_eq!(bits(&run.trades.exit_price), bits(&[99.0]));
    assert_eq!(run.trades.exit_time_us, vec![bar_time(1) + 2 * SECOND]);
}

#[test]
fn the_entry_bars_first_price_cannot_trigger_its_own_trade() {
    // Price 99 is the entry bar's first price, so it is the fill and not a trigger.
    let scenario = Scenario::hourly(&[(100.0, 100.0, 100.0), (100.0, 100.0, 99.0)])
        .enter(0, 1)
        .levels(0, 99.0, f64::NAN);
    let tape = Tape::new(&[(1, &[99.0, 99.5])]);

    let run = scenario.run(&tape);

    assert_eq!(
        run.trades.exit_bar,
        vec![-1],
        "no later price reached the stop"
    );
    assert_eq!(*tape.calls.borrow(), vec![1]);
}

#[test]
fn a_wrong_side_long_entry_is_rejected_and_reported() {
    let scenario = Scenario::hourly(&[(100.0, 100.0, 100.0), (100.0, 101.0, 99.0)])
        .enter(0, 1)
        .levels(0, 100.0, f64::NAN);
    let tape = Tape::new(&[]);

    let run = scenario.run(&tape);

    assert!(run.trades.entry_bar.is_empty());
    assert_eq!(run.rejected.bar, vec![1]);
    assert_eq!(run.rejected.side, vec![1]);
    assert_eq!(bits(&run.rejected.fill_price), bits(&[100.0]));
    assert_eq!(bits(&run.rejected.stop_price), bits(&[100.0]));
    assert!(run.rejected.target_price[0].is_nan());
    assert!(tape.calls.borrow().is_empty());
}

#[test]
fn a_wrong_side_short_entry_is_rejected_and_reported() {
    let scenario = Scenario::hourly(&[(100.0, 100.0, 100.0), (100.0, 101.0, 99.0)])
        .enter(0, -1)
        .levels(0, f64::NAN, 100.0);
    let tape = Tape::new(&[]);

    let run = scenario.run(&tape);

    assert!(run.trades.entry_bar.is_empty());
    assert_eq!(run.rejected.bar, vec![1]);
    assert_eq!(run.rejected.side, vec![-1]);
    assert!(run.rejected.stop_price[0].is_nan());
    assert_eq!(bits(&run.rejected.target_price), bits(&[100.0]));
}

#[test]
fn the_source_is_called_once_for_each_bar_that_passes_the_screen() {
    // Bar 2 passes the screen and no price reaches the stop, so the trade stays open. Bar 3's
    // range misses both levels and bar 4 passes again.
    let scenario = Scenario::hourly(&[
        (100.0, 100.0, 100.0),
        (100.0, 100.0, 100.0),
        (100.0, 100.0, 94.0),
        (100.0, 101.0, 100.0),
        (100.0, 100.0, 90.0),
    ])
    .enter(0, 1)
    .levels(0, 95.0, 110.0);
    let tape = Tape::new(&[(2, &[100.0, 96.0]), (4, &[90.0])]);

    let run = scenario.run(&tape);

    assert_eq!(*tape.calls.borrow(), vec![2, 4]);
    assert_eq!(run.trades.exit_bar, vec![4]);
    assert_eq!(bits(&run.trades.exit_price), bits(&[90.0]));
}

#[test]
fn an_empty_source_is_an_error_naming_the_bar() {
    let (scenario, tape) = (
        Scenario::hourly(&[
            (100.0, 100.0, 100.0),
            (100.0, 100.0, 100.0),
            (100.0, 100.0, 94.0),
        ])
        .enter(0, 1)
        .levels(0, 95.0, 110.0),
        Tape::new(&[(2, &[])]),
    );

    let err = scenario
        .run_with(Some(&tape), &Scenario::config(None))
        .expect_err("a bar that passes the screen needs prices");

    assert_eq!(
        err,
        CandleError::IntrabarSource {
            bar: 2,
            reason: "returned no prices".to_string(),
        }
    );
}

#[test]
fn a_failing_source_is_an_error_naming_the_bar() {
    let scenario = Scenario::hourly(&[
        (100.0, 100.0, 100.0),
        (100.0, 100.0, 100.0),
        (100.0, 100.0, 94.0),
    ])
    .enter(0, 1)
    .levels(0, 95.0, 110.0);

    let err = scenario
        .run_with(Some(&Failing), &Scenario::config(None))
        .expect_err("the source fails on bar 2");

    assert_eq!(
        err,
        CandleError::IntrabarSource {
            bar: 2,
            reason: "feed down".to_string(),
        }
    );
}

#[test]
fn a_day_trade_protected_trade_closes_at_the_session_close_when_no_level_trades() {
    // Bars at 10:00, 11:00 and 12:00; 12:00 is the last bar of the day and its range misses
    // both levels, so the source is never called and the trade closes at the close.
    let mut scenario = Scenario::hourly(&[
        (100.0, 100.0, 100.0),
        (100.0, 100.0, 100.0),
        (100.0, 105.0, 100.0),
    ])
    .enter(0, 1)
    .levels(0, 95.0, 110.0);
    scenario.close = Some(vec![100.0, 100.0, 103.0]);
    let tape = Tape::new(&[]);

    let run = scenario
        .run_with(Some(&tape), &Scenario::config(Some(day_trade())))
        .expect("valid series");

    assert_eq!(run.trades.exit_bar, vec![2]);
    assert_eq!(bits(&run.trades.exit_price), bits(&[103.0]));
    assert_eq!(run.trades.exit_reason, vec![ExitReason::EndOfDay.code()]);
    assert_eq!(run.trades.exit_time_us, vec![-1]);
    assert!(tape.calls.borrow().is_empty());
}

#[test]
fn the_force_close_bar_closes_at_its_open_and_never_calls_the_source() {
    // The third bar is at 17:00. Its range would pass the screen, but section A closes first.
    let mut scenario = Scenario::hourly(&[
        (100.0, 100.0, 100.0),
        (100.0, 100.0, 100.0),
        (92.0, 93.0, 90.0),
    ])
    .enter(0, 1)
    .levels(0, 95.0, 110.0);
    scenario.time_us[2] = 17 * HOUR;
    let tape = Tape::new(&[]);

    let run = scenario
        .run_with(Some(&tape), &Scenario::config(Some(day_trade())))
        .expect("valid series");

    assert_eq!(run.trades.exit_bar, vec![2]);
    assert_eq!(bits(&run.trades.exit_price), bits(&[92.0]));
    assert_eq!(run.trades.exit_reason, vec![ExitReason::EndOfDay.code()]);
    assert!(tape.calls.borrow().is_empty());
}

#[test]
fn protective_columns_require_an_intrabar_source() {
    let scenario = Scenario::hourly(&[(100.0, 100.0, 100.0), (100.0, 100.0, 100.0)])
        .enter(0, 1)
        .levels(0, 95.0, 110.0);

    let err = scenario
        .run_with(None, &Scenario::config(None))
        .expect_err("protective columns need a source");

    assert_eq!(
        err,
        CandleError::InvalidConfig {
            field: "protective",
            reason: "requires intrabar, open, high and low".to_string(),
        }
    );
}

#[test]
fn a_non_positive_level_is_rejected_by_column_and_bar() {
    let scenario = Scenario::hourly(&[(100.0, 100.0, 100.0), (100.0, 100.0, 100.0)])
        .enter(0, 1)
        .levels(0, -1.0, f64::NAN);

    let err = scenario
        .run_with(Some(&Tape::new(&[])), &Scenario::config(None))
        .expect_err("a stop of -1 is not a price");

    assert_eq!(
        err,
        CandleError::InvalidSignal {
            column: "stop_price",
            bar: 0,
            reason: "must be NaN or a finite positive price",
        }
    );
}

#[test]
fn a_double_run_with_protective_columns_is_identical() {
    let (first, _) = long_into_bar_two((100.0, 111.0, 94.0), &[100.0, 94.0, 111.0]);
    let (second, _) = long_into_bar_two((100.0, 111.0, 94.0), &[100.0, 94.0, 111.0]);

    assert_eq!(format!("{first:?}"), format!("{second:?}"));
}

fn day_trade() -> DayTradeWindow {
    DayTradeWindow {
        entry_start_us: 9 * HOUR,
        entry_end_us: 16 * HOUR,
        force_close_us: 17 * HOUR,
    }
}
