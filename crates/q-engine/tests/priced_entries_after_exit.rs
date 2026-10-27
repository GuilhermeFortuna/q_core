#![forbid(unsafe_code)]

use std::cell::RefCell;

use q_engine::{
    run_candle, CandleConfig, CandleInputs, IntrabarPrices, IntrabarSource, ProtectiveColumns,
    SignalColumns, Sizing,
};

struct Tape(Vec<f64>, RefCell<Vec<usize>>);

impl IntrabarSource for Tape {
    fn prices(&self, bar: usize) -> Result<IntrabarPrices, String> {
        self.1.borrow_mut().push(bar);
        Ok(IntrabarPrices {
            time_us: (0..self.0.len())
                .map(|i| bar as i64 * 10 + i as i64)
                .collect(),
            price: self.0.clone(),
        })
    }
}

fn run(ticks: &[f64], new_price: f64, new_stop: f64) -> q_engine::CandleRun {
    run_case(ticks, new_price, new_stop, 1, 95.0, f64::NAN)
}

fn run_case(
    ticks: &[f64],
    new_price: f64,
    new_stop: f64,
    side: i8,
    old_stop: f64,
    old_target: f64,
) -> q_engine::CandleRun {
    let tape = Tape(ticks.to_vec(), RefCell::new(Vec::new()));
    let time = [0, 10];
    let open = [100.0, 100.0];
    let high = [101.0, 110.0];
    let low = [99.0, 90.0];
    let entry = [side, side];
    let exit_long = [false; 2];
    let strength = [1.0; 2];
    let volatility = [1.0; 2];
    let stop = [old_stop, new_stop];
    let target = [old_target, f64::NAN];
    let entry_price = [f64::NAN, new_price];
    let none = |_: &str| None;
    let inputs = CandleInputs {
        time_us: &time,
        open: Some(&open),
        high: Some(&high),
        low: Some(&low),
        close: None,
        signals: SignalColumns {
            entry: &entry,
            exit_long: &exit_long,
            exit_short: &exit_long,
            strength: &strength,
            bar_index: None,
        },
        volatility: Some(&volatility),
        tradable: None,
        columns: &none,
        protective: Some(ProtectiveColumns {
            stop_price: &stop,
            target_price: &target,
            entry_price: Some(&entry_price),
        }),
        intrabar: Some(&tape),
    };
    run_candle(
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
        },
    )
    .unwrap()
}

#[test]
fn earlier_touch_is_rejected_after_same_bar_stop() {
    let result = run(&[100.0, 95.0, 99.0], 100.0, 90.0);
    assert_eq!(result.trades.entry_bar, vec![1]);
    assert_eq!(result.trades.exit_price, vec![95.0]);
    assert_eq!(result.rejected.bar, vec![1]);
    assert_eq!(result.rejected.reason, vec![1]);
}

#[test]
fn entry_without_levels_is_still_rejected_when_it_has_no_post_exit_touch() {
    let result = run(&[100.0, 95.0, 99.0], 100.0, f64::NAN);
    assert_eq!(result.trades.entry_bar, vec![1]);
    assert_eq!(result.rejected.reason, vec![1]);
}

#[test]
fn touch_at_or_after_exit_opens_and_resolves_levels_after_touch() {
    let result = run(&[100.0, 95.0, 100.0, 89.0], 100.0, 90.0);
    assert_eq!(result.trades.entry_bar, vec![1, 1]);
    assert_eq!(result.trades.exit_price, vec![95.0, 89.0]);
    assert_eq!(result.trades.exit_time_us, vec![11, 13]);
}

#[test]
fn target_exit_and_short_stop_exit_constrain_the_next_priced_entry() {
    let target_exit = run_case(&[100.0, 106.0, 99.0, 89.0], 100.0, 90.0, 1, f64::NAN, 105.0);
    assert_eq!(target_exit.trades.entry_bar, vec![1, 1]);
    assert_eq!(target_exit.trades.exit_price, vec![105.0, 89.0]);
    assert_eq!(target_exit.trades.exit_time_us, vec![11, 13]);

    let short_stop = run_case(
        &[100.0, 105.0, 99.0, 111.0],
        100.0,
        110.0,
        -1,
        105.0,
        f64::NAN,
    );
    assert_eq!(short_stop.trades.side, vec![-1, -1]);
    assert_eq!(short_stop.trades.entry_bar, vec![1, 1]);
    assert_eq!(short_stop.trades.exit_price, vec![105.0, 111.0]);
    assert_eq!(short_stop.trades.exit_time_us, vec![11, 13]);
}
