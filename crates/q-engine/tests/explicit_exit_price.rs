#![forbid(unsafe_code)]

use std::collections::BTreeMap;

use q_engine::{
    run_candle_with_callback, BarSignals, CandleConfig, CandleError, CandleInputs, ExitParams,
    IntrabarPrices, IntrabarSource, PositionSnapshot, RestingKind, RestingOrder, SignalColumns,
    Sizing,
};

const SECOND: i64 = 1_000_000;

struct Tape(BTreeMap<usize, Vec<f64>>);

impl IntrabarSource for Tape {
    fn prices(&self, bar: usize) -> Result<IntrabarPrices, String> {
        let price = self
            .0
            .get(&bar)
            .cloned()
            .ok_or_else(|| "missing bar".to_string())?;
        Ok(IntrabarPrices {
            time_us: (0..price.len())
                .map(|tick| bar as i64 * 10 * SECOND + tick as i64 * SECOND)
                .collect(),
            price,
        })
    }
}

struct EqualTimestampTape(Tape);

impl IntrabarSource for EqualTimestampTape {
    fn prices(&self, bar: usize) -> Result<IntrabarPrices, String> {
        let mut prices = self.0.prices(bar)?;
        let Some(&first_time) = prices.time_us.first() else {
            return Ok(prices);
        };
        prices.time_us.fill(first_time);
        Ok(prices)
    }
}

fn run(
    entry: &[i8],
    open: &[f64],
    high: &[f64],
    low: &[f64],
    tape: Option<&dyn IntrabarSource>,
    strategy: impl FnMut(usize, &[PositionSnapshot]) -> Result<BarSignals, CandleError>,
) -> Result<q_engine::CandleRun, CandleError> {
    let n = entry.len();
    let exit_long = vec![false; n];
    let exit_short = vec![false; n];
    let strength = entry
        .iter()
        .map(|value| if *value == 0 { 0.0 } else { 1.0 })
        .collect::<Vec<_>>();
    let time_us = (0..n)
        .map(|bar| bar as i64 * 10 * SECOND)
        .collect::<Vec<_>>();
    let no_columns = |_: &str| None;
    let inputs = CandleInputs {
        time_us: &time_us,
        open: Some(open),
        high: Some(high),
        low: Some(low),
        close: None,
        signals: SignalColumns {
            entry,
            exit_long: &exit_long,
            exit_short: &exit_short,
            strength: &strength,
            bar_index: None,
        },
        volatility: None,
        tradable: None,
        columns: &no_columns,
        protective: None,
        intrabar: tape,
    };
    run_candle_with_callback(
        &inputs,
        &CandleConfig {
            initial_capital: 1_000.0,
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
        },
        strategy,
    )
}

#[test]
fn signal_close_uses_the_first_trade_that_reaches_the_price() {
    let tape = Tape(BTreeMap::from([(2, vec![105.0, 103.0, 101.0])]));
    let run = run(
        &[1, 0, 0, 0],
        &[100.0, 100.0, 100.0, 120.0],
        &[100.0, 100.0, 106.0, 120.0],
        &[100.0, 100.0, 99.0, 120.0],
        Some(&tape),
        |bar, _| {
            Ok(match bar {
                0 => BarSignals {
                    entry: 1,
                    strength: 1.0,
                    ..BarSignals::default()
                },
                2 => BarSignals {
                    exit_long: true,
                    exit_price: Some(103.0),
                    ..BarSignals::default()
                },
                _ => BarSignals::default(),
            })
        },
    )
    .unwrap();
    assert_eq!(run.trades.exit_price, vec![103.0]);
    assert_eq!(run.trades.exit_time_us, vec![21 * SECOND]);
}

#[test]
fn priced_entry_after_signal_close_starts_at_the_close_tick() {
    let tape = Tape(BTreeMap::from([(1, vec![100.0, 95.0, 99.0])]));
    let result = run(
        &[1, 0, 0],
        &[100.0, 100.0, 120.0],
        &[100.0, 101.0, 120.0],
        &[100.0, 90.0, 120.0],
        Some(&tape),
        |bar, _| {
            Ok(match bar {
                0 => BarSignals {
                    entry: 1,
                    strength: 1.0,
                    ..BarSignals::default()
                },
                1 => BarSignals {
                    entry: 1,
                    strength: 1.0,
                    entry_price: Some(100.0),
                    exit_long: true,
                    exit_price: Some(95.0),
                    ..BarSignals::default()
                },
                _ => BarSignals::default(),
            })
        },
    )
    .unwrap();
    assert_eq!(result.trades.exit_price, vec![95.0]);
    assert_eq!(result.rejected.bar, vec![1]);
    assert_eq!(result.rejected.reason, vec![1]);
}

#[test]
fn a_later_bar_signal_close_uses_that_bars_start_after_a_late_entry_touch() {
    let tape = Tape(BTreeMap::from([
        (1, vec![100.0, 95.0, 100.0]),
        (2, vec![101.0]),
    ]));
    let result = run(
        &[1, 0, 0],
        &[100.0, 100.0, 100.0],
        &[100.0, 101.0, 101.0],
        &[100.0, 90.0, 99.0],
        Some(&tape),
        |bar, _| {
            Ok(match bar {
                0 => BarSignals {
                    entry: 1,
                    strength: 1.0,
                    ..BarSignals::default()
                },
                1 => BarSignals {
                    entry: 1,
                    strength: 1.0,
                    entry_price: Some(100.0),
                    exit_long: true,
                    exit_price: Some(95.0),
                    ..BarSignals::default()
                },
                2 => BarSignals {
                    exit_long: true,
                    exit_price: Some(101.0),
                    ..BarSignals::default()
                },
                _ => BarSignals::default(),
            })
        },
    )
    .unwrap();
    assert_eq!(result.trades.exit_bar, vec![1, 2]);
    assert_eq!(result.trades.exit_price, vec![95.0, 101.0]);
}

#[test]
fn close_scan_starts_at_a_resting_position_fill_time() {
    let tape = Tape(BTreeMap::from([(1, vec![101.0, 102.0, 103.0, 101.0])]));
    let result = run(
        &[0, 0, 0],
        &[100.0, 100.0, 120.0],
        &[100.0, 104.0, 120.0],
        &[100.0, 99.0, 120.0],
        Some(&tape),
        |bar, _| match bar {
            0 => Ok(BarSignals {
                entry: 1,
                strength: 1.0,
                resting: Some(RestingOrder {
                    kind: RestingKind::Stop,
                    price: 102.0,
                    expire_bars: None,
                }),
                ..BarSignals::default()
            }),
            1 => Ok(BarSignals {
                exit_long: true,
                exit_price: Some(101.0),
                ..BarSignals::default()
            }),
            _ => Ok(BarSignals::default()),
        },
    )
    .unwrap();
    assert_eq!(result.trades.entry_time_us, vec![11 * SECOND]);
    assert_eq!(result.trades.exit_time_us, vec![13 * SECOND]);
    assert_eq!(result.trades.exit_price, vec![101.0]);
}

#[test]
fn equal_timestamp_prices_before_a_resting_fill_are_not_scanned_for_close() {
    let tape = EqualTimestampTape(Tape(BTreeMap::from([(1, vec![100.0, 102.0])])));
    let result = run(
        &[0, 0, 0],
        &[100.0, 100.0, 120.0],
        &[100.0, 105.0, 120.0],
        &[100.0, 95.0, 120.0],
        Some(&tape),
        |bar, _| match bar {
            0 => Ok(BarSignals {
                entry: 1,
                strength: 1.0,
                resting: Some(RestingOrder {
                    kind: RestingKind::Stop,
                    price: 102.0,
                    expire_bars: None,
                }),
                ..BarSignals::default()
            }),
            1 => Ok(BarSignals {
                exit_long: true,
                exit_price: Some(100.0),
                ..BarSignals::default()
            }),
            _ => Ok(BarSignals::default()),
        },
    )
    .unwrap();

    assert_eq!(result.trades.entry_time_us, vec![10 * SECOND]);
    assert_eq!(result.trades.exit_bar, vec![-1]);
}

#[test]
fn a_resting_fill_print_itself_cannot_fill_a_priced_close() {
    let tape = Tape(BTreeMap::from([(1, vec![102.0])]));
    let result = run(
        &[0, 0, 0],
        &[100.0, 100.0, 120.0],
        &[100.0, 105.0, 120.0],
        &[100.0, 95.0, 120.0],
        Some(&tape),
        |bar, _| match bar {
            0 => Ok(BarSignals {
                entry: 1,
                strength: 1.0,
                resting: Some(RestingOrder {
                    kind: RestingKind::Stop,
                    price: 102.0,
                    expire_bars: None,
                }),
                ..BarSignals::default()
            }),
            1 => Ok(BarSignals {
                exit_long: true,
                exit_price: Some(102.0),
                ..BarSignals::default()
            }),
            _ => Ok(BarSignals::default()),
        },
    )
    .unwrap();

    assert_eq!(result.trades.entry_time_us, vec![10 * SECOND]);
    assert_eq!(result.trades.exit_bar, vec![-1]);
}

#[test]
fn short_signal_close_fills_at_the_requested_price_when_the_tape_trades_through_it() {
    let tape = Tape(BTreeMap::from([(2, vec![99.0, 102.0, 104.0])]));
    let result = run(
        &[-1, 0, 0, 0],
        &[100.0, 100.0, 100.0, 80.0],
        &[100.0, 100.0, 105.0, 80.0],
        &[100.0, 100.0, 95.0, 80.0],
        Some(&tape),
        |bar, _| {
            Ok(match bar {
                0 => BarSignals {
                    entry: -1,
                    strength: 1.0,
                    ..BarSignals::default()
                },
                2 => BarSignals {
                    exit_short: true,
                    exit_price: Some(103.0),
                    ..BarSignals::default()
                },
                _ => BarSignals::default(),
            })
        },
    )
    .unwrap();
    assert_eq!(result.trades.exit_price, vec![103.0]);
    assert_eq!(result.trades.exit_time_us, vec![22 * SECOND]);
}

#[test]
fn long_close_above_the_scan_start_fills_when_the_tape_rises_to_the_price() {
    let tape = Tape(BTreeMap::from([(2, vec![98.0, 101.0, 104.0, 102.0])]));
    let result = run(
        &[1, 0, 0, 0],
        &[100.0, 100.0, 98.0, 120.0],
        &[100.0, 100.0, 104.0, 120.0],
        &[100.0, 100.0, 98.0, 120.0],
        Some(&tape),
        |bar, _| {
            Ok(match bar {
                0 => BarSignals {
                    entry: 1,
                    strength: 1.0,
                    ..BarSignals::default()
                },
                2 => BarSignals {
                    exit_long: true,
                    exit_price: Some(103.0),
                    ..BarSignals::default()
                },
                _ => BarSignals::default(),
            })
        },
    )
    .unwrap();
    assert_eq!(result.trades.exit_price, vec![103.0]);
    assert_eq!(result.trades.exit_time_us, vec![22 * SECOND]);
}

#[test]
fn short_close_below_the_scan_start_fills_when_the_tape_falls_to_the_price() {
    let tape = Tape(BTreeMap::from([(2, vec![102.0, 99.0, 96.0])]));
    let result = run(
        &[-1, 0, 0, 0],
        &[100.0, 100.0, 102.0, 80.0],
        &[100.0, 100.0, 102.0, 80.0],
        &[100.0, 100.0, 96.0, 80.0],
        Some(&tape),
        |bar, _| {
            Ok(match bar {
                0 => BarSignals {
                    entry: -1,
                    strength: 1.0,
                    ..BarSignals::default()
                },
                2 => BarSignals {
                    exit_short: true,
                    exit_price: Some(97.0),
                    ..BarSignals::default()
                },
                _ => BarSignals::default(),
            })
        },
    )
    .unwrap();
    assert_eq!(result.trades.exit_price, vec![97.0]);
    assert_eq!(result.trades.exit_time_us, vec![22 * SECOND]);
}

#[test]
fn a_close_at_the_first_scanned_price_fills_on_that_trade() {
    let tape = Tape(BTreeMap::from([(2, vec![100.0, 104.0, 99.0])]));
    let result = run(
        &[1, 0, 0, 0],
        &[100.0, 100.0, 100.0, 120.0],
        &[100.0, 100.0, 104.0, 120.0],
        &[100.0, 100.0, 99.0, 120.0],
        Some(&tape),
        |bar, _| {
            Ok(match bar {
                0 => BarSignals {
                    entry: 1,
                    strength: 1.0,
                    ..BarSignals::default()
                },
                2 => BarSignals {
                    exit_long: true,
                    exit_price: Some(100.0),
                    ..BarSignals::default()
                },
                _ => BarSignals::default(),
            })
        },
    )
    .unwrap();
    assert_eq!(result.trades.exit_price, vec![100.0]);
    assert_eq!(result.trades.exit_time_us, vec![20 * SECOND]);
}

#[test]
fn protective_stop_that_fills_first_wins_over_the_decision_exit_price() {
    let tape = Tape(BTreeMap::from([(1, vec![100.0, 94.0, 100.0])]));
    let result = run(
        &[0, 0, 0],
        &[100.0, 100.0, 120.0],
        &[100.0, 101.0, 120.0],
        &[100.0, 90.0, 120.0],
        Some(&tape),
        |bar, _| {
            Ok(match bar {
                0 => BarSignals {
                    entry: 1,
                    strength: 1.0,
                    levels: Some((95.0, f64::NAN)),
                    ..BarSignals::default()
                },
                1 => BarSignals {
                    exit_long: true,
                    exit_price: Some(96.0),
                    ..BarSignals::default()
                },
                _ => BarSignals::default(),
            })
        },
    )
    .unwrap();
    assert_eq!(result.trades.exit_price, vec![94.0]);
    assert_eq!(result.trades.exit_reason, vec![14]);
}

#[test]
fn an_unreached_decision_price_does_not_queue_a_next_open_close() {
    let tape = Tape(BTreeMap::from([(2, vec![105.0, 103.0, 101.0])]));
    let result = run(
        &[0, 0, 0, 0],
        &[100.0, 100.0, 100.0, 120.0],
        &[100.0, 100.0, 106.0, 120.0],
        &[100.0, 100.0, 99.0, 120.0],
        Some(&tape),
        |bar, _| {
            Ok(match bar {
                0 => BarSignals {
                    entry: 1,
                    strength: 1.0,
                    ..BarSignals::default()
                },
                2 => BarSignals {
                    exit_long: true,
                    exit_price: Some(100.0),
                    ..BarSignals::default()
                },
                _ => BarSignals::default(),
            })
        },
    )
    .unwrap();
    assert_eq!(result.trades.exit_bar, vec![-1]);
}

#[test]
fn exit_price_validation_names_the_bar_and_column() {
    let tape = Tape(BTreeMap::from([(1, vec![100.0])]));
    let outside = run(
        &[1, 0],
        &[100.0, 100.0],
        &[100.0, 110.0],
        &[100.0, 90.0],
        Some(&tape),
        |bar, _| {
            Ok(match bar {
                0 => BarSignals {
                    entry: 1,
                    strength: 1.0,
                    ..BarSignals::default()
                },
                1 => BarSignals {
                    exit_long: true,
                    exit_price: Some(111.0),
                    ..BarSignals::default()
                },
                _ => BarSignals::default(),
            })
        },
    );
    assert!(matches!(
        outside,
        Err(CandleError::InvalidSignal {
            column: "exit_price",
            bar: 1,
            ..
        })
    ));

    let missing_source = run(
        &[1, 0],
        &[100.0, 100.0],
        &[100.0, 110.0],
        &[100.0, 90.0],
        None,
        |bar, _| {
            Ok(match bar {
                0 => BarSignals {
                    entry: 1,
                    strength: 1.0,
                    ..BarSignals::default()
                },
                1 => BarSignals {
                    exit_long: true,
                    exit_price: Some(105.0),
                    ..BarSignals::default()
                },
                _ => BarSignals::default(),
            })
        },
    );
    assert!(matches!(
        missing_source,
        Err(CandleError::InvalidSignal {
            column: "exit_price",
            ..
        })
    ));

    let invalid = run(
        &[1, 0],
        &[100.0, 100.0],
        &[100.0, 110.0],
        &[100.0, 90.0],
        None,
        |bar, _| {
            Ok(match bar {
                0 => BarSignals {
                    entry: 1,
                    strength: 1.0,
                    ..BarSignals::default()
                },
                1 => BarSignals {
                    exit_long: true,
                    exit_price: Some(f64::INFINITY),
                    ..BarSignals::default()
                },
                _ => BarSignals::default(),
            })
        },
    );
    assert!(matches!(
        invalid,
        Err(CandleError::InvalidSignal {
            column: "exit_price",
            bar: 1,
            ..
        })
    ));
}
