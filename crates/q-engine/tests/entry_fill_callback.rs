#![forbid(unsafe_code)]

use q_engine::{
    run_candle_with_callbacks, CandleConfig, CandleError, CandleInputs, CandleRun, EntryFilledFn,
    ExitReason, IntrabarPrices, IntrabarSource, ProtectiveColumns, RuntimeCallbacks, SignalColumns,
    Sizing,
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

#[derive(Debug)]
struct Failure;

impl From<CandleError> for Failure {
    fn from(err: CandleError) -> Self {
        panic!("kernel error: {err:?}")
    }
}

/// A long priced entry at 100 on bar 1 with a stop at `stop` and a target at 106. Bar 2 opens at
/// 100 and trades through the target later in the bar.
fn run<'a>(
    entry_bar_ticks: &[f64],
    stop: f64,
    entry_filled: Option<&'a mut EntryFilledFn<'a, Failure>>,
) -> CandleRun {
    let tape = Tape(vec![
        vec![100.0],
        entry_bar_ticks.to_vec(),
        vec![100.0, 103.0, 107.0],
    ]);
    let time = [0, 10, 20];
    let open = [100.0, 100.0, 100.0];
    let high = [100.0, 101.0, 107.0];
    let low = [100.0, 97.0, 100.0];
    let entry = [0, 1, 0];
    let exits = [false; 3];
    let strength = [1.0; 3];
    let stop_price = [f64::NAN, stop, f64::NAN];
    let target_price = [f64::NAN, 106.0, f64::NAN];
    let entry_price = [f64::NAN, 100.0, f64::NAN];
    let none = |_: &str| None;
    let inputs = CandleInputs {
        time_us: &time,
        open: Some(&open),
        high: Some(&high),
        low: Some(&low),
        close: None,
        signals: SignalColumns {
            entry: &entry,
            exit_long: &exits,
            exit_short: &exits,
            strength: &strength,
            bar_index: None,
        },
        volatility: None,
        tradable: None,
        columns: &none,
        protective: Some(ProtectiveColumns {
            stop_price: &stop_price,
            target_price: &target_price,
            entry_price: Some(&entry_price),
        }),
        intrabar: Some(&tape),
    };
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
            strategy: None,
            entry: None,
            entry_filled,
        },
    )
    .unwrap()
}

#[test]
fn a_close_requested_after_the_fill_exits_at_the_next_open_before_its_target() {
    let mut calls = Vec::new();
    let mut close = |bar: usize, positions: &[q_engine::PositionSnapshot]| {
        calls.push((bar, positions.len(), positions[0].entry_bar));
        Ok::<_, Failure>(true)
    };
    let result = run(&[100.0, 101.0, 99.0], 95.0, Some(&mut close));
    assert_eq!(calls, vec![(1, 1, 1)]);
    assert_eq!(result.trades.exit_bar, vec![2]);
    assert_eq!(result.trades.exit_price, vec![100.0]);
    assert_eq!(result.trades.exit_reason, vec![ExitReason::Signal.code()]);
}

#[test]
fn without_a_close_the_trade_keeps_its_protective_levels() {
    let mut keep = |_: usize, _: &[q_engine::PositionSnapshot]| Ok::<_, Failure>(false);
    let kept = run(&[100.0, 101.0, 99.0], 95.0, Some(&mut keep));
    let absent = run(&[100.0, 101.0, 99.0], 95.0, None);
    for result in [kept, absent] {
        assert_eq!(result.trades.exit_bar, vec![2]);
        assert_eq!(result.trades.exit_price, vec![106.0]);
        assert_eq!(
            result.trades.exit_reason,
            vec![ExitReason::TakeProfit.code()]
        );
    }
}

#[test]
fn a_trade_stopped_out_on_its_entry_bar_is_not_offered() {
    let mut calls = 0;
    let mut close = |_: usize, _: &[q_engine::PositionSnapshot]| {
        calls += 1;
        Ok::<_, Failure>(true)
    };
    let result = run(&[100.0, 101.0, 97.0], 98.0, Some(&mut close));
    assert_eq!(calls, 0);
    assert_eq!(result.trades.exit_bar, vec![1]);
    assert_eq!(result.trades.exit_reason, vec![ExitReason::StopLoss.code()]);
}
