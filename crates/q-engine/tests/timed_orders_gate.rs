#![forbid(unsafe_code)]

//! Gates for timed orders, expiry instant, and placement latency (Q-138 Acceptance 3).
//! - expiry by instant;
//! - timed exit filled intrabar at first trade at or after instant;
//! - timed exit where instant is before/at bar open (fills at bar open);
//! - a timed exit on a bar without a trade after the instant (fills on next bar's first trade);
//! - a non-zero latency that skips trades before close plus latency.

use std::collections::BTreeMap;

use q_engine::{
    run_candle_with_callbacks, BarSignals, CandleConfig, CandleError, CandleInputs, CandleRun,
    ExitReason, IntrabarPrices, IntrabarSource, PositionSnapshot, RestingKind, RestingOrder,
    RuntimeCallbacks, SignalColumns, Sizing, StrategyFn,
};

const SECOND: i64 = 1_000_000;
const MINUTE: i64 = 60 * SECOND;
const HOUR: i64 = 60 * MINUTE;

#[derive(Default)]
struct TestTape {
    bars: BTreeMap<usize, IntrabarPrices>,
}

impl TestTape {
    fn new(bars: &[(usize, &[(i64, f64)])]) -> Self {
        let bars = bars
            .iter()
            .map(|&(bar, ticks)| {
                let time_us = ticks.iter().map(|&(t, _)| t).collect();
                let price = ticks.iter().map(|&(_, p)| p).collect();
                (bar, IntrabarPrices { time_us, price })
            })
            .collect();
        Self { bars }
    }
}

impl IntrabarSource for TestTape {
    fn prices(&self, bar: usize) -> Result<IntrabarPrices, String> {
        self.bars
            .get(&bar)
            .cloned()
            .ok_or_else(|| format!("no tape for bar {bar}"))
    }
}

fn config(latency_us: i64) -> CandleConfig {
    CandleConfig {
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
        latency_us,
    }
}

type Decide = Box<dyn FnMut(usize, &[PositionSnapshot]) -> Result<BarSignals, CandleError>>;

struct Fixture {
    time_us: Vec<i64>,
    open: Vec<f64>,
    high: Vec<f64>,
    low: Vec<f64>,
    close: Vec<f64>,
    tape: TestTape,
    latency_us: i64,
}

impl Fixture {
    fn run(&self, mut strategy: Decide, entry: Option<Decide>) -> CandleRun {
        let n = self.time_us.len();
        let entries = vec![0; n];
        let exits = vec![false; n];
        let strength = vec![0.0; n];
        let none = |_: &str| None;
        let inputs = CandleInputs {
            time_us: &self.time_us,
            open: Some(&self.open),
            high: Some(&self.high),
            low: Some(&self.low),
            close: Some(&self.close),
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
            intrabar: Some(&self.tape),
        };
        let mut entry = entry;
        run_candle_with_callbacks(
            &inputs,
            &config(self.latency_us),
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
}

#[test]
fn expiry_by_instant_cancels_resting_order() {
    let expire_at = 11 * HOUR + 30 * SECOND;
    let fixture = Fixture {
        time_us: vec![10 * HOUR, 11 * HOUR],
        open: vec![100.0, 100.0],
        high: vec![101.0, 106.0],
        low: vec![99.0, 99.0],
        close: vec![100.0, 105.0],
        tape: TestTape::new(&[
            (0, &[(10 * HOUR, 100.0)]),
            // Tick 0: 100.0 at 11:00:00
            // Tick 1: 101.0 at 11:00:20
            // Tick 2: 105.0 at 11:00:40 (after expire_at 11:00:30)
            (
                1,
                &[
                    (11 * HOUR, 100.0),
                    (11 * HOUR + 20 * SECOND, 101.0),
                    (11 * HOUR + 40 * SECOND, 105.0),
                ],
            ),
        ]),
        latency_us: 0,
    };

    let result = fixture.run(
        Box::new(|_bar, _positions| Ok(BarSignals::default())),
        Some(Box::new(move |bar, _positions| match bar {
            0 => Ok(BarSignals {
                entry: 1,
                strength: 1.0,
                resting: Some(RestingOrder {
                    kind: RestingKind::Stop,
                    price: 104.0,
                    expire_bars: None,
                    expire_time_us: Some(expire_at),
                }),
                ..BarSignals::default()
            }),
            _ => Ok(BarSignals::default()),
        })),
    );

    // 104.0 was only touched at 11:00:40, after expiry instant 11:00:30.
    // Order was expired and cancelled, so no trade opened!
    assert_eq!(result.trades.entry_bar.len(), 0);
}

#[test]
fn timed_exit_filled_intrabar() {
    let exit_at = 11 * HOUR + 30 * SECOND;
    let fixture = Fixture {
        time_us: vec![10 * HOUR, 11 * HOUR],
        open: vec![100.0, 100.0],
        high: vec![101.0, 105.0],
        low: vec![99.0, 99.0],
        close: vec![100.0, 103.0],
        tape: TestTape::new(&[
            (0, &[(10 * HOUR, 100.0)]),
            (
                1,
                &[
                    (11 * HOUR, 100.0),
                    (11 * HOUR + 20 * SECOND, 101.0),
                    (11 * HOUR + 35 * SECOND, 103.5),
                    (11 * HOUR + 50 * SECOND, 105.0),
                ],
            ),
        ]),
        latency_us: 0,
    };

    let result = fixture.run(
        Box::new(move |bar, _positions| match bar {
            0 => Ok(BarSignals {
                timed_exit: Some(exit_at),
                ..BarSignals::default()
            }),
            _ => Ok(BarSignals::default()),
        }),
        Some(Box::new(|bar, _positions| match bar {
            0 => Ok(BarSignals {
                entry: 1,
                strength: 1.0,
                ..BarSignals::default()
            }),
            _ => Ok(BarSignals::default()),
        })),
    );

    // Trade entered at bar 1 open (100.0).
    // Timed exit at 11:00:30 filled at first trade at or after instant: tick 2 (11:00:35, price 103.5).
    assert_eq!(result.trades.entry_bar, vec![1]);
    assert_eq!(result.trades.entry_price, vec![100.0]);
    assert_eq!(result.trades.exit_bar, vec![1]);
    assert_eq!(result.trades.exit_price, vec![103.5]);
    assert_eq!(result.trades.exit_time_us, vec![11 * HOUR + 35 * SECOND]);
    assert_eq!(result.trades.exit_reason, vec![ExitReason::Signal.code()]);
}

#[test]
fn timed_exit_at_bar_open() {
    let exit_at = 12 * HOUR; // Exactly at bar 2 open time
    let fixture = Fixture {
        time_us: vec![10 * HOUR, 11 * HOUR, 12 * HOUR],
        open: vec![100.0, 102.0, 104.0],
        high: vec![101.0, 105.0, 106.0],
        low: vec![99.0, 101.0, 103.0],
        close: vec![100.0, 103.0, 105.0],
        tape: TestTape::new(&[
            (0, &[(10 * HOUR, 100.0)]),
            (1, &[(11 * HOUR, 102.0), (11 * HOUR + 20 * SECOND, 103.0)]),
            (2, &[(12 * HOUR, 104.0), (12 * HOUR + 20 * SECOND, 105.0)]),
        ]),
        latency_us: 0,
    };

    let result = fixture.run(
        Box::new(move |bar, _positions| match bar {
            0 => Ok(BarSignals {
                timed_exit: Some(exit_at),
                ..BarSignals::default()
            }),
            _ => Ok(BarSignals::default()),
        }),
        Some(Box::new(|bar, _positions| match bar {
            0 => Ok(BarSignals {
                entry: 1,
                strength: 1.0,
                ..BarSignals::default()
            }),
            _ => Ok(BarSignals::default()),
        })),
    );

    // Trade was entered on bar 1 open (102.0).
    // Held through bar 1.
    // On bar 2, first trade is at 12:00 >= exit_at (12:00).
    // Timed exit closes at bar 2 open (104.0).
    assert_eq!(result.trades.entry_bar, vec![1]);
    assert_eq!(result.trades.exit_bar, vec![2]);
    assert_eq!(result.trades.exit_price, vec![104.0]);
    assert_eq!(result.trades.exit_reason, vec![ExitReason::Signal.code()]);
}

#[test]
fn timed_exit_bar_without_trade_after_instant_fills_on_next_bar() {
    let exit_at = 11 * HOUR + 45 * MINUTE; // 11:45
    let fixture = Fixture {
        time_us: vec![10 * HOUR, 11 * HOUR, 12 * HOUR],
        open: vec![100.0, 100.0, 104.0],
        high: vec![101.0, 103.0, 106.0],
        low: vec![99.0, 99.0, 103.0],
        close: vec![100.0, 102.0, 105.0],
        tape: TestTape::new(&[
            (0, &[(10 * HOUR, 100.0)]),
            // Bar 1 has trades only up to 11:10
            (1, &[(11 * HOUR, 100.0), (11 * HOUR + 10 * MINUTE, 102.0)]),
            // Bar 2 first trade is at 12:00 (which is after 11:45)
            (2, &[(12 * HOUR, 104.0), (12 * HOUR + 5 * MINUTE, 105.0)]),
        ]),
        latency_us: 0,
    };

    let result = fixture.run(
        Box::new(move |bar, _positions| match bar {
            0 => Ok(BarSignals {
                timed_exit: Some(exit_at),
                ..BarSignals::default()
            }),
            _ => Ok(BarSignals::default()),
        }),
        Some(Box::new(|bar, _positions| match bar {
            0 => Ok(BarSignals {
                entry: 1,
                strength: 1.0,
                ..BarSignals::default()
            }),
            _ => Ok(BarSignals::default()),
        })),
    );

    // Trade entered on bar 1 open.
    // Bar 1 trades end at 11:10, before 11:45. Trade stays open!
    // On bar 2, first trade is at 12:00 >= 11:45. Fills at bar 2 open (104.0)!
    assert_eq!(result.trades.entry_bar, vec![1]);
    assert_eq!(result.trades.exit_bar, vec![2]);
    assert_eq!(result.trades.exit_price, vec![104.0]);
    assert_eq!(result.trades.exit_reason, vec![ExitReason::Signal.code()]);
}

#[test]
fn placement_latency_skips_trades_before_close_plus_latency() {
    let latency_us = 30 * SECOND;
    let fixture = Fixture {
        time_us: vec![10 * HOUR, 11 * HOUR],
        open: vec![100.0, 100.0],
        high: vec![101.0, 106.0],
        low: vec![99.0, 99.0],
        close: vec![100.0, 105.0],
        tape: TestTape::new(&[
            (0, &[(10 * HOUR, 100.0)]),
            // Bar 0 closes at 11:00. Latency is 30s, so earliest fill is 11:00:30.
            // Tick 0: 100.0 at 11:00:00
            // Tick 1: 105.0 at 11:00:15 (touches stop 104.0, but within latency -> SKIPPED!)
            // Tick 2: 101.0 at 11:00:25
            // Tick 3: 104.5 at 11:00:45 (after latency -> FILLS at 104.5!)
            (
                1,
                &[
                    (11 * HOUR, 100.0),
                    (11 * HOUR + 15 * SECOND, 105.0),
                    (11 * HOUR + 25 * SECOND, 101.0),
                    (11 * HOUR + 45 * SECOND, 104.5),
                ],
            ),
        ]),
        latency_us,
    };

    let result = fixture.run(
        Box::new(|_bar, _positions| Ok(BarSignals::default())),
        Some(Box::new(|bar, _positions| match bar {
            0 => Ok(BarSignals {
                entry: 1,
                strength: 1.0,
                resting: Some(RestingOrder {
                    kind: RestingKind::Stop,
                    price: 104.0,
                    expire_bars: None,
                    expire_time_us: None,
                }),
                ..BarSignals::default()
            }),
            _ => Ok(BarSignals::default()),
        })),
    );

    // Did NOT fill at tick 1 (105.0 at 11:00:15) because of latency.
    // Filled at tick 3 (104.5 at 11:00:45)!
    assert_eq!(result.trades.entry_bar, vec![1]);
    assert_eq!(result.trades.entry_price, vec![104.5]);
    assert_eq!(result.trades.entry_time_us, vec![11 * HOUR + 45 * SECOND]);
}
