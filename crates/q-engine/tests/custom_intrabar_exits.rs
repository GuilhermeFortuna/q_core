#![forbid(unsafe_code)]

//! Custom intrabar exits and runtime entry levels. Each series is hand-built; the tape source
//! serves one bar's trade prices and records the bars the kernel asked for.

use std::cell::RefCell;
use std::collections::BTreeMap;

use q_engine::{
    run_candle_with_callbacks, BarSignals, CandleConfig, CandleError, CandleInputs, CandleRun,
    ExitCallbacks, ExitParams, ExitReason, ExitScreenFn, ExitTickFn, IntrabarPrices,
    IntrabarSource, PositionSnapshot, ProtectiveColumns, RuntimeCallbacks, SignalColumns, Sizing,
    StrategyFn,
};

const HOUR: i64 = 3_600_000_000;
const SECOND: i64 = 1_000_000;

fn bar_time(bar: usize) -> i64 {
    (10 + bar as i64) * HOUR
}

struct Tape {
    bars: BTreeMap<usize, Vec<f64>>,
    calls: RefCell<Vec<usize>>,
}

impl Tape {
    fn new(bars: &[(usize, &[f64])]) -> Self {
        Self {
            bars: bars
                .iter()
                .map(|&(bar, prices)| (bar, prices.to_vec()))
                .collect(),
            calls: RefCell::new(Vec::new()),
        }
    }
}

impl IntrabarSource for Tape {
    fn prices(&self, bar: usize) -> Result<IntrabarPrices, String> {
        self.calls.borrow_mut().push(bar);
        let prices = self
            .bars
            .get(&bar)
            .ok_or_else(|| format!("no tape for bar {bar}"))?;
        Ok(IntrabarPrices {
            time_us: (0..prices.len())
                .map(|i| bar_time(bar) + i as i64 * SECOND)
                .collect(),
            price: prices.clone(),
        })
    }
}

/// Hourly bars from 10:00 with optional entries and per-bar levels; `NaN` means no level.
struct Series {
    time_us: Vec<i64>,
    open: Vec<f64>,
    high: Vec<f64>,
    low: Vec<f64>,
    entry: Vec<i8>,
    strength: Vec<f64>,
    stop: Vec<f64>,
    target: Vec<f64>,
}

impl Series {
    fn hourly(bars: &[(f64, f64, f64)]) -> Self {
        let n = bars.len();
        Self {
            time_us: (0..n).map(bar_time).collect(),
            open: bars.iter().map(|bar| bar.0).collect(),
            high: bars.iter().map(|bar| bar.1).collect(),
            low: bars.iter().map(|bar| bar.2).collect(),
            entry: vec![0; n],
            strength: vec![0.0; n],
            stop: vec![f64::NAN; n],
            target: vec![f64::NAN; n],
        }
    }

    fn enter(mut self, bar: usize, side: i8) -> Self {
        self.entry[bar] = side;
        self.strength[bar] = 1.0;
        self
    }

    fn levels(mut self, bar: usize, stop: f64, target: f64) -> Self {
        self.stop[bar] = stop;
        self.target[bar] = target;
        self
    }

    fn config() -> CandleConfig {
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
            day_trade: None,
            force_close_at_end: false,
        }
    }
}

fn run<'a>(
    series: &Series,
    intrabar: Option<&dyn IntrabarSource>,
    strategy: Option<&'a mut StrategyFn<'a, CandleError>>,
    exit: Option<ExitCallbacks<'a, CandleError>>,
) -> Result<CandleRun, CandleError> {
    let n = series.time_us.len();
    let no_exit = vec![false; n];
    let no_columns = |_: &str| None;
    let inputs = CandleInputs {
        time_us: &series.time_us,
        open: Some(&series.open),
        high: Some(&series.high),
        low: Some(&series.low),
        close: None,
        signals: SignalColumns {
            entry: &series.entry,
            exit_long: &no_exit,
            exit_short: &no_exit,
            strength: &series.strength,
            bar_index: None,
        },
        volatility: None,
        tradable: None,
        columns: &no_columns,
        protective: intrabar.is_some().then_some(ProtectiveColumns {
            stop_price: &series.stop,
            target_price: &series.target,
        }),
        intrabar,
    };
    run_candle_with_callbacks(
        &inputs,
        &Series::config(),
        RuntimeCallbacks { strategy, exit },
    )
}

/// A long entry queued on bar 0 fills at bar 1's open of 100. Bar 2 has the range
/// `(open, high, low)` and the given prices, and bar 3 opens at 120, so a next-open fill is visible.
fn long_into_bar_two(
    range: (f64, f64, f64),
    prices: &[f64],
    stop: f64,
    target: f64,
) -> (Series, Tape) {
    let series = Series::hourly(&[
        (100.0, 100.0, 100.0),
        (100.0, 100.0, 100.0),
        range,
        (120.0, 120.0, 120.0),
    ])
    .enter(0, 1)
    .levels(0, stop, target);
    (series, Tape::new(&[(2, prices)]))
}

#[test]
fn a_custom_exit_fills_at_the_first_confirmed_price_not_at_the_next_open() {
    let (series, tape) = long_into_bar_two(
        (100.0, 106.0, 99.0),
        &[101.0, 104.0, 105.0, 106.0],
        95.0,
        130.0,
    );
    let screen = |bar: usize, _: &[PositionSnapshot]| Ok::<_, CandleError>(bar == 2);
    let tick = |_: usize, _: usize, _: i64, price: f64, _: &[PositionSnapshot]| {
        Ok::<_, CandleError>(price >= 105.0)
    };
    let (mut screen, mut tick) = (screen, tick);
    let run = run(
        &series,
        Some(&tape),
        None,
        Some(ExitCallbacks {
            screen: &mut screen as &mut ExitScreenFn<'_, CandleError>,
            tick: &mut tick as &mut ExitTickFn<'_, CandleError>,
        }),
    )
    .unwrap();

    assert_eq!(run.trades.exit_bar, vec![2]);
    assert_eq!(run.trades.exit_price, vec![105.0]);
    assert_eq!(run.trades.exit_reason, vec![ExitReason::Signal.code()]);
    assert_eq!(run.trades.exit_time_us, vec![bar_time(2) + 2 * SECOND]);
    assert_eq!(*tape.calls.borrow(), vec![2]);
}

struct CustomRun {
    result: Result<CandleRun, CandleError>,
    /// `(bar, open positions)` for each screen call.
    screened: Vec<(usize, usize)>,
    /// `(bar, tick, price)` for each tick call.
    ticked: Vec<(usize, usize, f64)>,
}

/// Runs a long entry on bar 0 with a screen that qualifies only `screen_bar` and a tick rule.
fn custom_exit_run(
    series: &Series,
    tape: &Tape,
    screen_bar: usize,
    tick_rule: impl Fn(f64) -> bool,
) -> CustomRun {
    let screened = RefCell::new(Vec::new());
    let ticked = RefCell::new(Vec::new());
    let mut screen = |bar: usize, positions: &[PositionSnapshot]| {
        screened.borrow_mut().push((bar, positions.len()));
        Ok::<_, CandleError>(bar == screen_bar)
    };
    let mut tick = |bar: usize, tick: usize, _: i64, price: f64, _: &[PositionSnapshot]| {
        ticked.borrow_mut().push((bar, tick, price));
        Ok::<_, CandleError>(tick_rule(price))
    };
    let result = run(
        series,
        Some(tape),
        None,
        Some(ExitCallbacks {
            screen: &mut screen as &mut ExitScreenFn<'_, CandleError>,
            tick: &mut tick as &mut ExitTickFn<'_, CandleError>,
        }),
    );
    CustomRun {
        result,
        screened: screened.into_inner(),
        ticked: ticked.into_inner(),
    }
}

#[test]
fn flat_candles_are_not_screened_and_a_screen_that_never_qualifies_reads_no_ticks() {
    let (series, tape) = long_into_bar_two((100.0, 106.0, 99.0), &[101.0], 95.0, 130.0);
    let CustomRun {
        result,
        screened,
        ticked,
    } = custom_exit_run(&series, &tape, usize::MAX, |_| true);
    let run = result.unwrap();

    assert_eq!(screened, vec![(1, 1), (2, 1), (3, 1)]);
    assert!(ticked.is_empty());
    assert!(tape.calls.borrow().is_empty());
    assert_eq!(run.trades.exit_bar, vec![-1]);
}

#[test]
fn a_screen_without_a_confirming_tick_does_not_fill() {
    let (series, tape) = long_into_bar_two((100.0, 104.0, 99.0), &[103.0, 104.0], 95.0, f64::NAN);
    let CustomRun { result, ticked, .. } =
        custom_exit_run(&series, &tape, 2, |price| price >= 105.0);
    let run = result.unwrap();

    assert_eq!(run.trades.exit_bar, vec![-1]);
    assert_eq!(ticked.len(), 2);
    assert_eq!(*tape.calls.borrow(), vec![2]);
}

#[test]
fn an_earlier_crossing_that_reverses_does_not_fill_and_the_confirmed_price_does() {
    let (series, tape) = long_into_bar_two((100.0, 106.0, 99.0), &[106.0, 103.0], 95.0, f64::NAN);
    let CustomRun { result, .. } = custom_exit_run(&series, &tape, 2, |price| price <= 103.0);
    let run = result.unwrap();

    assert_eq!(run.trades.exit_price, vec![103.0]);
    assert_eq!(run.trades.exit_time_us, vec![bar_time(2) + SECOND]);
}

#[test]
fn a_protective_stop_precedes_a_custom_exit_at_the_same_tick() {
    let (series, tape) = long_into_bar_two((100.0, 100.0, 94.0), &[94.0, 90.0], 95.0, 130.0);
    let CustomRun { result, ticked, .. } = custom_exit_run(&series, &tape, 2, |_| true);
    let run = result.unwrap();

    assert_eq!(run.trades.exit_reason, vec![ExitReason::StopLoss.code()]);
    assert_eq!(run.trades.exit_price, vec![94.0]);
    assert!(ticked.is_empty());
}

#[test]
fn an_earlier_custom_exit_precedes_a_later_protective_stop() {
    let (series, tape) = long_into_bar_two((100.0, 101.0, 94.0), &[101.0, 94.0], 95.0, 130.0);
    let CustomRun { result, .. } = custom_exit_run(&series, &tape, 2, |price| price >= 101.0);
    let run = result.unwrap();

    assert_eq!(run.trades.exit_reason, vec![ExitReason::Signal.code()]);
    assert_eq!(run.trades.exit_price, vec![101.0]);
    assert_eq!(run.trades.exit_time_us, vec![bar_time(2)]);
}

#[test]
fn an_entry_cannot_be_closed_by_the_custom_exit_on_its_own_entry_tick() {
    let series = Series::hourly(&[
        (100.0, 100.0, 100.0),
        (100.0, 100.0, 100.0),
        (100.0, 101.0, 100.0),
        (120.0, 120.0, 120.0),
    ])
    .enter(1, 1);
    let tape = Tape::new(&[(2, &[100.0, 101.0])]);
    let CustomRun { result, ticked, .. } = custom_exit_run(&series, &tape, 2, |_| true);
    let run = result.unwrap();

    assert_eq!(run.trades.entry_bar, vec![2]);
    assert_eq!(run.trades.exit_price, vec![101.0]);
    assert_eq!(run.trades.exit_time_us, vec![bar_time(2) + SECOND]);
    assert_eq!(ticked, vec![(2, 0, 100.0), (2, 1, 101.0)]);
}

#[test]
fn runtime_entry_levels_reach_the_kernel_through_the_strategy_callback() {
    let series = Series::hourly(&[
        (100.0, 100.0, 100.0),
        (100.0, 100.0, 100.0),
        (100.0, 101.0, 94.0),
        (120.0, 120.0, 120.0),
    ]);
    let tape = Tape::new(&[(2, &[101.0, 94.0])]);
    let mut strategy = |bar: usize, _: &[PositionSnapshot]| {
        Ok::<_, CandleError>(BarSignals {
            entry: i8::from(bar == 0),
            strength: f64::from(u8::from(bar == 0)),
            levels: (bar == 0).then_some((95.0, 130.0)),
            ..BarSignals::default()
        })
    };
    let run = run(
        &series,
        Some(&tape),
        Some(&mut strategy as &mut StrategyFn<'_, CandleError>),
        None,
    )
    .unwrap();

    assert_eq!(run.trades.exit_reason, vec![ExitReason::StopLoss.code()]);
    assert_eq!(run.trades.exit_price, vec![94.0]);
}

#[test]
fn runtime_levels_without_an_intrabar_source_are_rejected() {
    let series = Series::hourly(&[(100.0, 100.0, 100.0), (100.0, 100.0, 100.0)]);
    let mut strategy = |bar: usize, _: &[PositionSnapshot]| {
        Ok::<_, CandleError>(BarSignals {
            entry: i8::from(bar == 0),
            strength: f64::from(u8::from(bar == 0)),
            levels: Some((95.0, f64::NAN)),
            ..BarSignals::default()
        })
    };
    let result = run(
        &series,
        None,
        Some(&mut strategy as &mut StrategyFn<'_, CandleError>),
        None,
    );

    assert!(matches!(
        result,
        Err(CandleError::InvalidConfig {
            field: "stop_price",
            ..
        })
    ));
}

#[test]
fn a_custom_exit_without_an_intrabar_source_is_rejected() {
    let series = Series::hourly(&[(100.0, 100.0, 100.0)]);
    let mut screen = |_: usize, _: &[PositionSnapshot]| Ok::<_, CandleError>(false);
    let mut tick =
        |_: usize, _: usize, _: i64, _: f64, _: &[PositionSnapshot]| Ok::<_, CandleError>(false);
    let result = run(
        &series,
        None,
        None,
        Some(ExitCallbacks {
            screen: &mut screen as &mut ExitScreenFn<'_, CandleError>,
            tick: &mut tick as &mut ExitTickFn<'_, CandleError>,
        }),
    );

    assert!(matches!(
        result,
        Err(CandleError::InvalidConfig {
            field: "exit_screen_callback",
            ..
        })
    ));
}

#[test]
fn a_runtime_level_must_be_a_finite_positive_price() {
    let series = Series::hourly(&[(100.0, 100.0, 100.0), (100.0, 100.0, 100.0)]);
    let tape = Tape::new(&[]);
    let mut strategy = |bar: usize, _: &[PositionSnapshot]| {
        Ok::<_, CandleError>(BarSignals {
            entry: i8::from(bar == 0),
            strength: f64::from(u8::from(bar == 0)),
            levels: Some((-1.0, 130.0)),
            ..BarSignals::default()
        })
    };
    let result = run(
        &series,
        Some(&tape),
        Some(&mut strategy as &mut StrategyFn<'_, CandleError>),
        None,
    );

    assert_eq!(
        result.err(),
        Some(CandleError::InvalidSignal {
            column: "stop_price",
            bar: 0,
            reason: "must be NaN or a finite positive price",
        })
    );
}
