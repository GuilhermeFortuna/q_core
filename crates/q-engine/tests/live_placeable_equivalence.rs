#![forbid(unsafe_code)]

//! Equivalence gates for live-placeable orders (Q-138 Acceptance 2).
//! Each gate pairs a legacy priced pattern with its replacement on the same fixture bars and ticks,
//! and requires equivalent ledgers:
//! 1. Priced entry list (bracket) against an OCO group of resting stops with 1-bar expiry.
//!    Includes same-bar exit, exact touch, and gap open.
//! 2. Priced close at the current bar's open once open passes a time against a timed exit at that time.
//! 3. Priced stop close at a level fixed by previous bar against a protective stop set by amendment at previous close.

use std::collections::BTreeMap;

use q_engine::{
    run_candle_with_callbacks, BarSignals, CandleConfig, CandleError, CandleInputs, CandleRun,
    EntryCandidate, ExitReason, IntrabarPrices, IntrabarSource, OcoOrder, PositionSnapshot,
    RestingKind, RuntimeCallbacks, Side, SignalColumns, Sizing, StrategyFn, TradeLedger,
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

fn assert_ledger_equivalent(actual: &TradeLedger, expected: &TradeLedger, check_exit_reason: bool) {
    assert_eq!(actual.entry_bar, expected.entry_bar, "entry_bar mismatch");
    assert_eq!(actual.exit_bar, expected.exit_bar, "exit_bar mismatch");
    assert_eq!(actual.side, expected.side, "side mismatch");
    assert_eq!(actual.quantity, expected.quantity, "quantity mismatch");
    assert_eq!(
        actual.entry_price, expected.entry_price,
        "entry_price mismatch"
    );
    assert_eq!(
        actual.exit_price, expected.exit_price,
        "exit_price mismatch"
    );
    assert_eq!(
        actual.exit_time_us, expected.exit_time_us,
        "exit_time_us mismatch"
    );
    assert_eq!(
        actual.commission, expected.commission,
        "commission mismatch"
    );
    assert_eq!(actual.pnl, expected.pnl, "pnl mismatch");
    if check_exit_reason {
        assert_eq!(
            actual.exit_reason, expected.exit_reason,
            "exit_reason mismatch"
        );
    }
}

// ---------------------------------------------------------------------------
// Gate 1: Priced entry list (bracket) vs OCO group of resting stops
// Includes: same-bar exit, exact touch of level, and gap open.
// ---------------------------------------------------------------------------

fn gate_1_fixture() -> Fixture {
    let time_us = vec![10 * HOUR, 11 * HOUR, 12 * HOUR];
    let open = vec![100.0, 100.0, 106.0];
    let high = vec![101.0, 103.0, 108.0];
    let low = vec![99.0, 89.0, 105.0];
    let close = vec![100.0, 102.0, 107.0];

    let tape = TestTape::new(&[
        (0, 10 * HOUR, &[100.0, 100.0]),
        (1, 11 * HOUR, &[100.0, 94.0, 98.0, 102.0, 103.0]),
        (2, 12 * HOUR, &[106.0, 108.0, 105.0]),
    ]);
    Fixture {
        time_us,
        open,
        high,
        low,
        close,
        tape,
    }
}

fn run_legacy_1_bracket() -> CandleRun {
    let fixture = gate_1_fixture();
    fixture.run(
        Box::new(|_bar, _positions| Ok(BarSignals::default())),
        Some(Box::new(|bar, _positions| match bar {
            0 => Ok(BarSignals {
                entry: 1,
                strength: 1.0,
                levels: Some((95.0, 110.0)),
                ..BarSignals::default()
            }),
            1 => Ok(BarSignals {
                entry: 1,
                strength: 1.0,
                entry_candidates: Some(vec![
                    EntryCandidate {
                        side: Side::Long,
                        price: 102.0,
                        levels: Some((97.0, 103.0)),
                    },
                    EntryCandidate {
                        side: Side::Short,
                        price: 90.0,
                        levels: None,
                    },
                ]),
                ..BarSignals::default()
            }),
            _ => Ok(BarSignals::default()),
        })),
    )
}

#[test]
fn test_legacy_form_1_bracket() {
    let result = run_legacy_1_bracket();
    // Recorded expected ledger from legacy kernel:
    // Trade 1: entered on bar 1 (open at 100.0), stopped out at 94.0 at tick 1 (same-bar exit!)
    // Trade 2: Candidate 0 filled at 102.0 on bar 1 (exact touch!), exited on bar 2 at 103.0 (target)
    assert_eq!(result.trades.entry_bar, vec![1, 1]);
    assert_eq!(result.trades.exit_bar, vec![1, 2]);
    assert_eq!(result.trades.side, vec![1, 1]);
    assert_eq!(result.trades.quantity, vec![1.0, 1.0]);
    assert_eq!(result.trades.entry_price, vec![100.0, 102.0]);
    assert_eq!(result.trades.exit_price, vec![94.0, 103.0]);
    assert_eq!(
        result.trades.exit_reason,
        vec![ExitReason::StopLoss.code(), ExitReason::TakeProfit.code()]
    );
    assert_eq!(
        result.trades.exit_time_us,
        vec![11 * HOUR + SECOND, 12 * HOUR]
    );
    assert_eq!(result.trades.pnl, vec![-6.0, 1.0]);
}

fn run_new_1_oco() -> CandleRun {
    let fixture = gate_1_fixture();
    fixture.run(
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
                        price: 102.0,
                        levels: Some((97.0, 103.0)),
                        expire_bars: Some(1),
                        expire_time_us: None,
                    },
                    OcoOrder {
                        side: Side::Short,
                        kind: RestingKind::Stop,
                        price: 90.0,
                        levels: None,
                        expire_bars: Some(1),
                        expire_time_us: None,
                    },
                ]),
                ..BarSignals::default()
            }),
            _ => Ok(BarSignals::default()),
        })),
    )
}

#[test]
fn test_equivalence_gate_1_oco_bracket() {
    let legacy = run_legacy_1_bracket();
    let new = run_new_1_oco();
    assert_ledger_equivalent(&new.trades, &legacy.trades, true);
}

// ---------------------------------------------------------------------------
// Gate 2: Priced close at open once open passes a time vs Timed exit
// Includes: gap open beyond target time, filled at the bar's open.
// ---------------------------------------------------------------------------

fn gate_2_fixture() -> Fixture {
    let time_us = vec![10 * HOUR, 11 * HOUR, 12 * HOUR];
    let open = vec![100.0, 102.0, 104.0];
    let high = vec![101.0, 103.0, 105.0];
    let low = vec![99.0, 101.0, 103.0];
    let close = vec![100.0, 102.0, 104.0];

    let tape = TestTape::new(&[
        (0, 10 * HOUR, &[100.0, 100.0]),
        (1, 11 * HOUR, &[102.0, 102.5, 103.0]),
        (2, 12 * HOUR, &[104.0, 104.5, 105.0]),
    ]);
    Fixture {
        time_us,
        open,
        high,
        low,
        close,
        tape,
    }
}

fn run_legacy_2_timed_close() -> CandleRun {
    let fixture = gate_2_fixture();
    fixture.run(
        Box::new(|bar, _positions| match bar {
            // On bar 2, open passes 11:30. Legacy requests priced close at current bar open 104.0.
            2 => Ok(BarSignals {
                exit_long: true,
                exit_prices: vec![104.0],
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
    )
}

#[test]
fn test_legacy_form_2_timed_close() {
    let result = run_legacy_2_timed_close();
    // Recorded expected ledger from legacy kernel:
    assert_eq!(result.trades.entry_bar, vec![1]);
    assert_eq!(result.trades.exit_bar, vec![2]);
    assert_eq!(result.trades.side, vec![1]);
    assert_eq!(result.trades.quantity, vec![1.0]);
    assert_eq!(result.trades.entry_price, vec![102.0]);
    assert_eq!(result.trades.exit_price, vec![104.0]);
    assert_eq!(result.trades.exit_reason, vec![ExitReason::Signal.code()]);
    assert_eq!(result.trades.exit_time_us, vec![12 * HOUR]);
    assert_eq!(result.trades.pnl, vec![2.0]);
}

fn run_new_2_timed_exit() -> CandleRun {
    let fixture = gate_2_fixture();
    fixture.run(
        Box::new(|bar, _positions| match bar {
            // Place timed exit at 11:30 at bar 0 (or bar 1) close.
            0 => Ok(BarSignals {
                timed_exit: Some(11 * HOUR + 30 * MINUTE),
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
    )
}

#[test]
fn test_equivalence_gate_2_timed_exit() {
    let legacy = run_legacy_2_timed_close();
    let new = run_new_2_timed_exit();
    assert_ledger_equivalent(&new.trades, &legacy.trades, true);
}

// ---------------------------------------------------------------------------
// Gate 3: Priced stop close vs amended protective stop
// Includes: exact touch of level at 95.0.
// ---------------------------------------------------------------------------

fn gate_3_fixture() -> Fixture {
    let time_us = vec![10 * HOUR, 11 * HOUR, 12 * HOUR];
    let open = vec![100.0, 100.0, 100.0];
    let high = vec![101.0, 101.0, 101.0];
    let low = vec![99.0, 99.0, 93.0];
    let close = vec![100.0, 100.0, 94.0];

    let tape = TestTape::new(&[
        (0, 10 * HOUR, &[100.0, 100.0]),
        (1, 11 * HOUR, &[100.0, 100.0]),
        (2, 12 * HOUR, &[100.0, 97.0, 95.0, 93.0]),
    ]);
    Fixture {
        time_us,
        open,
        high,
        low,
        close,
        tape,
    }
}

fn run_legacy_3_priced_stop_close() -> CandleRun {
    let fixture = gate_3_fixture();
    fixture.run(
        Box::new(|bar, _positions| match bar {
            2 => Ok(BarSignals {
                exit_long: true,
                exit_prices: vec![95.0],
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
    )
}

#[test]
fn test_legacy_form_3_priced_stop_close() {
    let result = run_legacy_3_priced_stop_close();
    // Recorded expected ledger from legacy kernel:
    assert_eq!(result.trades.entry_bar, vec![1]);
    assert_eq!(result.trades.exit_bar, vec![2]);
    assert_eq!(result.trades.side, vec![1]);
    assert_eq!(result.trades.quantity, vec![1.0]);
    assert_eq!(result.trades.entry_price, vec![100.0]);
    assert_eq!(result.trades.exit_price, vec![95.0]);
    assert_eq!(result.trades.exit_time_us, vec![12 * HOUR + 2 * SECOND]);
    assert_eq!(result.trades.pnl, vec![-5.0]);
}

fn run_new_3_amended_stop() -> CandleRun {
    let fixture = gate_3_fixture();
    fixture.run(
        Box::new(|bar, _positions| match bar {
            1 => Ok(BarSignals {
                amend_stop: Some(95.0),
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
    )
}

#[test]
fn test_equivalence_gate_3_amended_stop() {
    let legacy = run_legacy_3_priced_stop_close();
    let new = run_new_3_amended_stop();
    assert_ledger_equivalent(&new.trades, &legacy.trades, false);
    assert_eq!(new.trades.exit_reason, vec![ExitReason::StopLoss.code()]);
}
