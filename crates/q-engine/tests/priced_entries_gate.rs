#![forbid(unsafe_code)]

//! Hand-computed cases for priced entries. An entry decided on a bar fills on that bar at its own
//! price, and its stop and target resolve from the bar's trade prices after the touch. Each series
//! is small enough to check by hand; the tape source records the bars the kernel asked for.

use std::cell::RefCell;
use std::collections::BTreeMap;

use q_engine::{
    run_candle, CandleConfig, CandleError, CandleInputs, CandleRun, ExitParams, ExitReason,
    IntrabarPrices, IntrabarSource, ProtectiveColumns, Side, SignalColumns, Sizing,
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

/// Hand-built hourly series with one optional priced entry per bar; `NaN` means no level or price.
struct Scenario {
    time_us: Vec<i64>,
    open: Vec<f64>,
    high: Vec<f64>,
    low: Vec<f64>,
    entry: Vec<i8>,
    exit_long: Vec<bool>,
    strength: Vec<f64>,
    volatility: Vec<f64>,
    stop: Vec<f64>,
    target: Vec<f64>,
    entry_price: Vec<f64>,
}

impl Scenario {
    /// One hourly bar per `(open, high, low)` from 10:00, with no signals.
    fn hourly(bars: &[(f64, f64, f64)]) -> Self {
        let n = bars.len();
        Self {
            time_us: (0..n).map(bar_time).collect(),
            open: bars.iter().map(|bar| bar.0).collect(),
            high: bars.iter().map(|bar| bar.1).collect(),
            low: bars.iter().map(|bar| bar.2).collect(),
            entry: vec![0; n],
            exit_long: vec![false; n],
            strength: vec![0.0; n],
            volatility: vec![1.0; n],
            stop: vec![f64::NAN; n],
            target: vec![f64::NAN; n],
            entry_price: vec![f64::NAN; n],
        }
    }

    /// Decides an entry on `bar` at `price`.
    fn enter(mut self, bar: usize, side: i8, price: f64) -> Self {
        self.entry_price[bar] = price;
        self.queue(bar, side)
    }

    /// Decides an entry on `bar` that fills at the next open, with no price.
    fn queue(mut self, bar: usize, side: i8) -> Self {
        self.entry[bar] = side;
        self.strength[bar] = 1.0;
        self
    }

    /// Decides a long exit on `bar`.
    fn exit_long(mut self, bar: usize) -> Self {
        self.exit_long[bar] = true;
        self
    }

    /// Sets the levels the entry decided on `bar` carries.
    fn levels(mut self, bar: usize, stop: f64, target: f64) -> Self {
        self.stop[bar] = stop;
        self.target[bar] = target;
        self
    }

    fn run(&self, intrabar: &dyn IntrabarSource) -> Result<CandleRun, CandleError> {
        self.run_sized(intrabar, fixed_quantity())
    }

    fn run_sized(
        &self,
        intrabar: &dyn IntrabarSource,
        sizing: Sizing,
    ) -> Result<CandleRun, CandleError> {
        let n = self.time_us.len();
        let no_exit = vec![false; n];
        let no_columns = |_: &str| None;
        let inputs = CandleInputs {
            time_us: &self.time_us,
            open: Some(&self.open),
            high: Some(&self.high),
            low: Some(&self.low),
            close: None,
            signals: SignalColumns {
                entry: &self.entry,
                exit_long: &self.exit_long,
                exit_short: &no_exit,
                strength: &self.strength,
                bar_index: None,
            },
            volatility: Some(&self.volatility),
            tradable: None,
            columns: &no_columns,
            protective: Some(ProtectiveColumns {
                stop_price: &self.stop,
                target_price: &self.target,
                entry_price: Some(&self.entry_price),
            }),
            intrabar: Some(intrabar),
        };
        run_candle(&inputs, &config(sizing))
    }
}

fn fixed_quantity() -> Sizing {
    Sizing::FixedQuantity {
        quantity: 1.0,
        scale_by_strength: false,
    }
}

fn config(sizing: Sizing) -> CandleConfig {
    CandleConfig {
        initial_capital: 10_000.0,
        point_value: 1.0,
        costs: None,
        sizing,
        holding_period_bars: None,
        exit_params: ExitParams::default(),
        day_trade: None,
        force_close_at_end: false,
    }
}

#[test]
fn a_priced_long_opens_on_its_deciding_bar_at_the_price() {
    // Bar 1 decides a long at 101, the high of bar 0. Bar 1 opens at 100 and trades through 101.
    let scenario =
        Scenario::hourly(&[(100.0, 101.0, 99.0), (100.0, 102.0, 99.5)]).enter(1, 1, 101.0);
    let tape = Tape::default();

    let run = scenario.run(&tape).expect("hand-built series is valid");

    assert_eq!(run.trades.entry_bar, vec![1]);
    assert_eq!(run.trades.side, vec![1]);
    assert_eq!(bits(&run.trades.entry_price), bits(&[101.0]));
    assert_eq!(run.trades.exit_bar, vec![-1]);
    assert!(tape.calls.borrow().is_empty());
}

#[test]
fn a_priced_long_below_the_open_fills_when_the_low_reaches_it() {
    let scenario = Scenario::hourly(&[(100.0, 101.0, 97.5)]).enter(0, 1, 98.0);

    let run = scenario
        .run(&Tape::default())
        .expect("hand-built series is valid");

    assert_eq!(run.trades.entry_bar, vec![0]);
    assert_eq!(bits(&run.trades.entry_price), bits(&[98.0]));
}

#[test]
fn a_priced_entry_outside_the_deciding_bar_range_ends_the_run_naming_the_bar() {
    let above = Scenario::hourly(&[(100.0, 101.0, 99.0)]).enter(0, 1, 102.0);
    let err = above.run(&Tape::default()).unwrap_err();
    assert_eq!(
        err,
        CandleError::EntryPriceOutsideRange {
            bar: 0,
            side: Side::Long,
            price: 102.0,
            low: 99.0,
            high: 101.0,
        }
    );

    let below = Scenario::hourly(&[(100.0, 101.0, 99.0)]).enter(0, -1, 98.5);
    let err = below.run(&Tape::default()).unwrap_err();
    assert_eq!(
        err,
        CandleError::EntryPriceOutsideRange {
            bar: 0,
            side: Side::Short,
            price: 98.5,
            low: 99.0,
            high: 101.0,
        }
    );
}

#[test]
fn a_priced_long_resolves_its_target_from_the_price_that_first_touches_its_price() {
    // Bar 0 opens at 99 and trades 99, 100, 99.5, 101.5. The touch is 100, the second price, so the
    // trade opens at 100 and the 101.5 print closes it at the 101 target on the same bar.
    let scenario = Scenario::hourly(&[(99.0, 101.5, 99.0)])
        .enter(0, 1, 100.0)
        .levels(0, f64::NAN, 101.0);
    let tape = Tape::new(&[(0, &[99.0, 100.0, 99.5, 101.5])]);

    let run = scenario.run(&tape).expect("hand-built series is valid");

    assert_eq!(run.trades.entry_bar, vec![0]);
    assert_eq!(bits(&run.trades.entry_price), bits(&[100.0]));
    assert_eq!(run.trades.exit_bar, vec![0]);
    assert_eq!(bits(&run.trades.exit_price), bits(&[101.0]));
    assert_eq!(run.trades.exit_reason, vec![ExitReason::TakeProfit.code()]);
    assert_eq!(run.trades.exit_time_us, vec![bar_time(0) + 3 * SECOND]);
    assert_eq!(*tape.calls.borrow(), vec![0]);
}

#[test]
fn prices_before_the_touch_are_ignored() {
    // The bar opens at 101.5 and trades 101.5 before the touch at 99. A target at 101 would fill
    // on the opening print if that print counted, so the exit must come from the later 101.5.
    let scenario = Scenario::hourly(&[(101.5, 101.5, 99.0)])
        .enter(0, 1, 100.0)
        .levels(0, f64::NAN, 101.0);
    let tape = Tape::new(&[(0, &[101.5, 99.0, 100.0, 101.5])]);

    let run = scenario.run(&tape).expect("hand-built series is valid");

    assert_eq!(run.trades.entry_bar, vec![0]);
    assert_eq!(bits(&run.trades.exit_price), bits(&[101.0]));
    assert_eq!(run.trades.exit_time_us, vec![bar_time(0) + 3 * SECOND]);
}

#[test]
fn a_priced_short_resolves_its_stop_from_prices_after_the_touch() {
    // Bar 0 opens at 101, above the short price of 100, so the touch is the first print at or
    // below 100. The 106 print then reaches the 105 stop and fills at the traded price.
    let scenario = Scenario::hourly(&[(101.0, 106.0, 99.0)])
        .enter(0, -1, 100.0)
        .levels(0, 105.0, 90.0);
    let tape = Tape::new(&[(0, &[101.0, 100.0, 106.0])]);

    let run = scenario.run(&tape).expect("hand-built series is valid");

    assert_eq!(run.trades.side, vec![-1]);
    assert_eq!(bits(&run.trades.entry_price), bits(&[100.0]));
    assert_eq!(bits(&run.trades.exit_price), bits(&[106.0]));
    assert_eq!(run.trades.exit_reason, vec![ExitReason::StopLoss.code()]);
    assert_eq!(run.trades.exit_time_us, vec![bar_time(0) + 2 * SECOND]);
}

#[test]
fn a_priced_long_whose_stop_is_at_its_price_is_rejected_with_its_bar_side_and_price() {
    let scenario = Scenario::hourly(&[(100.0, 101.0, 99.0)])
        .enter(0, 1, 100.0)
        .levels(0, 100.0, f64::NAN);
    let tape = Tape::default();

    let run = scenario.run(&tape).expect("hand-built series is valid");

    assert!(run.trades.entry_bar.is_empty());
    assert_eq!(run.rejected.bar, vec![0]);
    assert_eq!(run.rejected.side, vec![1]);
    assert_eq!(bits(&run.rejected.fill_price), bits(&[100.0]));
    assert_eq!(bits(&run.rejected.stop_price), bits(&[100.0]));
    assert!(tape.calls.borrow().is_empty());
}

#[test]
fn a_priced_entry_is_not_queued_for_the_next_bar() {
    let scenario =
        Scenario::hourly(&[(100.0, 101.0, 99.0), (100.0, 101.0, 99.0)]).enter(0, 1, 100.0);

    let run = scenario
        .run(&Tape::default())
        .expect("hand-built series is valid");

    assert_eq!(run.trades.entry_bar, vec![0]);
    assert_eq!(run.trades.entry_price.len(), 1);
    // The decision is still traced on the bar that made it.
    assert_eq!(run.trace.entry, vec![1, 0]);
}

#[test]
fn a_priced_entry_survives_the_exit_queued_with_it() {
    // Bar 0 queues a long that opens on bar 1 at its open of 100. Bar 1 closes that long at bar
    // 2's open and, on the same decision, opens a short at 99. Only the long may close on bar 2.
    let scenario = Scenario::hourly(&[
        (100.0, 101.0, 99.0),
        (100.0, 101.0, 98.0),
        (97.0, 98.0, 96.0),
    ])
    .queue(0, 1)
    .enter(1, -1, 99.0)
    .exit_long(1);
    // Inverse volatility of 1 with a 1% target on 10,000 sizes each trade at floor(100 / price), so
    // both trades are one contract. Without a contract cap the short fits beside the open long.
    let sizing = Sizing::InverseVolatility {
        target_volatility_pct: 1.0,
        point_value: 1.0,
        min_contracts: 0,
        max_contracts: None,
        scale_by_strength: false,
    };

    let run = scenario
        .run_sized(&Tape::default(), sizing)
        .expect("hand-built series is valid");

    assert_eq!(run.trades.side, vec![1, -1]);
    assert_eq!(run.trades.entry_bar, vec![1, 1]);
    assert_eq!(run.trades.exit_bar, vec![2, -1]);
    assert_eq!(bits(&run.trades.exit_price), bits(&[97.0, f64::NAN]));
}

#[test]
fn a_bar_whose_range_cannot_reach_a_level_after_the_touch_does_not_call_the_source() {
    // Neither level is inside the bar's range of 98 to 105, so the source is never asked.
    let scenario = Scenario::hourly(&[(100.0, 105.0, 98.0)])
        .enter(0, 1, 100.0)
        .levels(0, 95.0, 110.0);
    let tape = Tape::default();

    let run = scenario.run(&tape).expect("hand-built series is valid");

    assert_eq!(run.trades.entry_bar, vec![0]);
    assert!(tape.calls.borrow().is_empty());
}
