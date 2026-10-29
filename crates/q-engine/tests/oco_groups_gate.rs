#![forbid(unsafe_code)]

//! Gates for OCO groups in the candle kernel (Q-138 Acceptance 3).
//! - an OCO group where the second member fills first;
//! - members filling on the same tick resolve in list order;
//! - a group replaced while working;
//! - Q-132 rule: members fill only at or after a same-bar exit.

use std::collections::BTreeMap;

use q_engine::{
    run_candle_with_callbacks, BarSignals, CandleConfig, CandleError, CandleInputs, CandleRun,
    ExitReason, IntrabarPrices, IntrabarSource, OcoOrder, PositionSnapshot, RestingKind,
    RuntimeCallbacks, Side, SignalColumns, Sizing, StrategyFn,
};

const SECOND: i64 = 1_000_000;
const MINUTE: i64 = 60 * SECOND;
const HOUR: i64 = 60 * MINUTE;

#[derive(Default)]
struct TestTape {
    bars: BTreeMap<usize, IntrabarPrices>,
}

impl TestTape {
    fn new(bars: &[(usize, i64, &[f64])]) -> Self {
        let bars = bars
            .iter()
            .map(|&(bar, start_us, prices)| {
                let time_us = (0..prices.len())
                    .map(|i| start_us + i as i64 * SECOND)
                    .collect();
                let prices = IntrabarPrices {
                    time_us,
                    price: prices.to_vec(),
                };
                (bar, prices)
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
        exit_params: Default::default(),
        day_trade: None,
        force_close_at_end: false,
        latency_us: 0,
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
            &config(),
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
fn oco_second_member_fills_first_and_cancels_other() {
    let fixture = Fixture {
        time_us: vec![10 * HOUR, 11 * HOUR],
        open: vec![100.0, 100.0],
        high: vec![101.0, 106.0],
        low: vec![99.0, 94.0],
        close: vec![100.0, 105.0],
        tape: TestTape::new(&[
            (0, 10 * HOUR, &[100.0, 100.0]),
            // First trades drop to 95.0 (fills member 1: sell stop), then rise to 105.0 (member 0 price)
            (1, 11 * HOUR, &[100.0, 97.0, 95.0, 101.0, 105.0]),
        ]),
    };

    let result = fixture.run(
        Box::new(|bar, _positions| match bar {
            0 => Ok(BarSignals {
                oco_group: Some(vec![
                    OcoOrder {
                        side: Side::Long,
                        kind: RestingKind::Stop,
                        price: 105.0,
                        levels: None,
                        expire_bars: None,
                        expire_time_us: None,
                    },
                    OcoOrder {
                        side: Side::Short,
                        kind: RestingKind::Stop,
                        price: 95.0,
                        levels: None,
                        expire_bars: None,
                        expire_time_us: None,
                    },
                ]),
                ..BarSignals::default()
            }),
            _ => Ok(BarSignals::default()),
        }),
        None,
    );

    // Only one trade opened, by member 1 (short at 95.0). Member 0 was cancelled.
    assert_eq!(result.trades.entry_bar, vec![1]);
    assert_eq!(result.trades.side, vec![-1]);
    assert_eq!(result.trades.entry_price, vec![95.0]);
    assert_eq!(result.trades.entry_candidate, vec![1]);
}

#[test]
fn oco_members_filling_on_same_tick_resolve_in_list_order() {
    let fixture = Fixture {
        time_us: vec![10 * HOUR, 11 * HOUR],
        open: vec![100.0, 100.0],
        high: vec![101.0, 106.0],
        low: vec![99.0, 99.0],
        close: vec![100.0, 106.0],
        tape: TestTape::new(&[
            (0, 10 * HOUR, &[100.0, 100.0]),
            // Bar 1 jumps from 100.0 directly to 106.0 on tick 1, passing both 102.0 and 104.0
            (1, 11 * HOUR, &[100.0, 106.0]),
        ]),
    };

    let result = fixture.run(
        Box::new(|bar, _positions| match bar {
            0 => Ok(BarSignals {
                oco_group: Some(vec![
                    OcoOrder {
                        side: Side::Long,
                        kind: RestingKind::Stop,
                        price: 104.0,
                        levels: None,
                        expire_bars: None,
                        expire_time_us: None,
                    },
                    OcoOrder {
                        side: Side::Long,
                        kind: RestingKind::Stop,
                        price: 102.0,
                        levels: None,
                        expire_bars: None,
                        expire_time_us: None,
                    },
                ]),
                ..BarSignals::default()
            }),
            _ => Ok(BarSignals::default()),
        }),
        None,
    );

    // Both reached on the jump to 106.0. Resolves in list order: member 0 wins.
    assert_eq!(result.trades.entry_bar, vec![1]);
    assert_eq!(result.trades.entry_price, vec![106.0]);
    assert_eq!(result.trades.entry_candidate, vec![0]);
}

#[test]
fn oco_group_replaced_while_working() {
    let fixture = Fixture {
        time_us: vec![10 * HOUR, 11 * HOUR, 12 * HOUR],
        open: vec![100.0, 100.0, 100.0],
        high: vec![101.0, 102.0, 105.0],
        low: vec![99.0, 98.0, 89.0],
        close: vec![100.0, 100.0, 104.0],
        tape: TestTape::new(&[
            (0, 10 * HOUR, &[100.0, 100.0]),
            (1, 11 * HOUR, &[100.0, 101.0, 99.0]),
            // Bar 2 reaches 104.0, then drops to 90.0
            (2, 12 * HOUR, &[100.0, 104.0, 90.0]),
        ]),
    };

    let result = fixture.run(
        Box::new(|bar, _positions| match bar {
            0 => Ok(BarSignals {
                // Group A: Buy stop 110, Sell stop 90
                oco_group: Some(vec![
                    OcoOrder {
                        side: Side::Long,
                        kind: RestingKind::Stop,
                        price: 110.0,
                        levels: None,
                        expire_bars: None,
                        expire_time_us: None,
                    },
                    OcoOrder {
                        side: Side::Short,
                        kind: RestingKind::Stop,
                        price: 90.0,
                        levels: None,
                        expire_bars: None,
                        expire_time_us: None,
                    },
                ]),
                ..BarSignals::default()
            }),
            1 => Ok(BarSignals {
                // Group B: Buy stop 104, Sell stop 96. Replaces Group A!
                oco_group: Some(vec![
                    OcoOrder {
                        side: Side::Long,
                        kind: RestingKind::Stop,
                        price: 104.0,
                        levels: None,
                        expire_bars: None,
                        expire_time_us: None,
                    },
                    OcoOrder {
                        side: Side::Short,
                        kind: RestingKind::Stop,
                        price: 96.0,
                        levels: None,
                        expire_bars: None,
                        expire_time_us: None,
                    },
                ]),
                ..BarSignals::default()
            }),
            _ => Ok(BarSignals::default()),
        }),
        None,
    );

    // Group B's member 0 fills at 104.0. Group A's member 1 (90.0) is not triggered.
    assert_eq!(result.trades.entry_bar, vec![2]);
    assert_eq!(result.trades.entry_price, vec![104.0]);
    assert_eq!(result.trades.side, vec![1]);
    assert_eq!(result.trades.entry_candidate, vec![0]);
}

#[test]
fn oco_applies_q132_after_exit_rule() {
    let fixture = Fixture {
        time_us: vec![10 * HOUR, 11 * HOUR],
        open: vec![100.0, 100.0],
        high: vec![101.0, 104.0],
        low: vec![99.0, 94.0],
        close: vec![100.0, 101.0],
        tape: TestTape::new(&[
            (0, 10 * HOUR, &[100.0, 100.0]),
            // Tick 0 touches 103.0 before exit at tick 1 (95.0), then tick 2 is 98.0
            (1, 11 * HOUR, &[103.0, 95.0, 98.0]),
        ]),
    };

    let result = fixture.run(
        Box::new(|_bar, _positions| Ok(BarSignals::default())),
        Some(Box::new(|bar, _positions| match bar {
            0 => Ok(BarSignals {
                entry: 1,
                strength: 1.0,
                levels: Some((95.0, 110.0)),
                oco_group: Some(vec![
                    OcoOrder {
                        side: Side::Long,
                        kind: RestingKind::Stop,
                        price: 103.0,
                        levels: None,
                        expire_bars: None,
                        expire_time_us: None,
                    },
                    OcoOrder {
                        side: Side::Short,
                        kind: RestingKind::Stop,
                        price: 90.0,
                        levels: None,
                        expire_bars: None,
                        expire_time_us: None,
                    },
                ]),
                ..BarSignals::default()
            }),
            _ => Ok(BarSignals::default()),
        })),
    );

    // Initial trade stopped out at 95.0 on bar 1.
    // Member 0 (103.0) was touched only at tick 0 before the exit, so Q-132 rejects filling it!
    assert_eq!(result.trades.entry_bar, vec![1]);
    assert_eq!(result.trades.exit_price, vec![95.0]);
    assert_eq!(result.trades.exit_reason, vec![ExitReason::StopLoss.code()]);
    // No second trade opened!
    assert_eq!(result.trades.entry_bar.len(), 1);
}
