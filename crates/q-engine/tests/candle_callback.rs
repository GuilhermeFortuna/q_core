use q_engine::{
    run_candle, run_candle_with_callback, BarSignals, CandleConfig, CandleError, CandleInputs,
    Costs, ExitParams, SignalColumns, Sizing,
};

#[test]
fn callback_sees_filled_positions_and_preserves_static_results() {
    let times = [0, 1, 2, 3, 4];
    let prices = [100., 101., 102., 103., 104.];
    let entry = [1, -1, 0, 0, 0];
    let close = [false, true, false, false, false];
    let strength = [1.; 5];
    let lookup = |_: &str| None;
    let inputs = CandleInputs {
        time_us: &times,
        open: Some(&prices),
        high: Some(&prices),
        low: Some(&prices),
        close: Some(&prices),
        signals: SignalColumns {
            entry: &entry,
            exit_long: &close,
            exit_short: &close,
            strength: &strength,
            bar_index: None,
        },
        volatility: None,
        tradable: None,
        columns: &lookup,
        protective: None,
        intrabar: None,
    };
    let config = CandleConfig {
        initial_capital: 10000.,
        point_value: 1.,
        costs: Some(Costs {
            per_contract: 2.,
            bps: 1.,
        }),
        sizing: Sizing::FixedQuantity {
            quantity: 2.,
            scale_by_strength: false,
        },
        holding_period_bars: None,
        exit_params: ExitParams::default(),
        day_trade: None,
        force_close_at_end: true,
    };
    let expected = run_candle(&inputs, &config).unwrap();
    let mut seen = Vec::new();
    let actual = run_candle_with_callback(&inputs, &config, |bar, positions| {
        seen.push(positions.to_vec());
        Ok::<_, CandleError>(BarSignals {
            entry: entry[bar],
            exit_long: close[bar],
            exit_short: close[bar],
            strength: strength[bar],
            levels: None,
            entry_price: None,
        })
    })
    .unwrap();
    assert!(seen[0].is_empty());
    assert_eq!(seen[1].len(), 1);
    assert_eq!(seen[1][0].entry_bar, 1);
    assert_eq!(seen[1][0].entry_price.to_bits(), 101_f64.to_bits());
    assert_eq!(seen[1][0].quantity.to_bits(), 2_f64.to_bits());
    assert_eq!(seen[2][0].entry_bar, 2);
    assert_eq!(actual.trades.entry_bar, expected.trades.entry_bar);
    assert_eq!(actual.trades.exit_bar, expected.trades.exit_bar);
    assert_eq!(actual.trades.pnl, expected.trades.pnl);
    assert_eq!(actual.trades.commission, expected.trades.commission);
    assert_eq!(actual.trace.entry, expected.trace.entry);
    assert_eq!(actual.trace.exit_reason, expected.trace.exit_reason);
}

#[test]
fn callback_error_aborts_run() {
    let lookup = |_: &str| None;
    let inputs = CandleInputs {
        time_us: &[0],
        open: Some(&[100.]),
        high: None,
        low: None,
        close: Some(&[100.]),
        signals: SignalColumns {
            entry: &[0],
            exit_long: &[false],
            exit_short: &[false],
            strength: &[0.],
            bar_index: None,
        },
        volatility: None,
        tradable: None,
        columns: &lookup,
        protective: None,
        intrabar: None,
    };
    let config = CandleConfig {
        initial_capital: 10000.,
        point_value: 1.,
        costs: None,
        sizing: Sizing::FixedQuantity {
            quantity: 1.,
            scale_by_strength: false,
        },
        holding_period_bars: None,
        exit_params: ExitParams::default(),
        day_trade: None,
        force_close_at_end: false,
    };
    let result = run_candle_with_callback(&inputs, &config, |_, _| {
        Err::<BarSignals, _>(CandleError::InvalidConfig {
            field: "callback",
            reason: "test failure".into(),
        })
    });
    assert!(matches!(
        result,
        Err(CandleError::InvalidConfig {
            field: "callback",
            ..
        })
    ));
}

#[test]
fn callback_observes_intrabar_closures_and_rejected_entries() {
    use q_engine::{IntrabarPrices, IntrabarSource, ProtectiveColumns};
    struct Tape;
    impl IntrabarSource for Tape {
        fn prices(&self, bar: usize) -> Result<IntrabarPrices, String> {
            Ok(IntrabarPrices {
                time_us: vec![bar as i64, bar as i64 + 1],
                price: vec![100., 98.],
            })
        }
    }
    let lookup = |_: &str| None;
    let inputs = CandleInputs {
        time_us: &[0, 10, 20],
        open: Some(&[100.; 3]),
        high: Some(&[101.; 3]),
        low: Some(&[98.; 3]),
        close: Some(&[100.; 3]),
        signals: SignalColumns {
            entry: &[0; 3],
            exit_long: &[false; 3],
            exit_short: &[false; 3],
            strength: &[0.; 3],
            bar_index: None,
        },
        volatility: None,
        tradable: None,
        columns: &lookup,
        protective: Some(ProtectiveColumns {
            stop_price: &[99., 101., f64::NAN],
            target_price: &[f64::NAN; 3],
            entry_price: None,
        }),
        intrabar: Some(&Tape),
    };
    let config = CandleConfig {
        initial_capital: 10000.,
        point_value: 1.,
        costs: None,
        sizing: Sizing::FixedQuantity {
            quantity: 1.,
            scale_by_strength: false,
        },
        holding_period_bars: None,
        exit_params: ExitParams::default(),
        day_trade: None,
        force_close_at_end: false,
    };
    let mut seen = Vec::new();
    let run = run_candle_with_callback(&inputs, &config, |bar, positions| {
        seen.push(positions.len());
        Ok::<_, CandleError>(BarSignals {
            entry: if bar < 2 { 1 } else { 0 },
            strength: 1.,
            ..BarSignals::default()
        })
    })
    .unwrap();
    assert_eq!(seen, vec![0, 0, 0]);
    assert_eq!(run.trades.exit_bar, vec![1]);
    assert_eq!(run.rejected.bar, vec![2]);
}

#[test]
fn callback_runs_on_warmup_force_close_and_final_bars() {
    use q_engine::DayTradeWindow;
    const HOUR: i64 = 3_600_000_000;
    let lookup = |_: &str| None;
    let inputs = CandleInputs {
        time_us: &[9 * HOUR, 10 * HOUR, 11 * HOUR, 12 * HOUR, 13 * HOUR],
        open: Some(&[100.; 5]),
        high: None,
        low: None,
        close: Some(&[100.; 5]),
        signals: SignalColumns {
            entry: &[0; 5],
            exit_long: &[false; 5],
            exit_short: &[false; 5],
            strength: &[0.; 5],
            bar_index: None,
        },
        volatility: None,
        tradable: Some(&[false, true, true, true, true]),
        columns: &lookup,
        protective: None,
        intrabar: None,
    };
    let config = CandleConfig {
        initial_capital: 10000.,
        point_value: 1.,
        costs: None,
        sizing: Sizing::FixedQuantity {
            quantity: 1.,
            scale_by_strength: false,
        },
        holding_period_bars: None,
        exit_params: ExitParams::default(),
        day_trade: Some(DayTradeWindow {
            entry_start_us: 10 * HOUR,
            entry_end_us: 11 * HOUR,
            force_close_us: 12 * HOUR,
        }),
        force_close_at_end: false,
    };
    let mut seen = Vec::new();
    let run = run_candle_with_callback(&inputs, &config, |_, positions| {
        seen.push(positions.len());
        Ok::<_, CandleError>(BarSignals {
            entry: 1,
            strength: 1.,
            ..BarSignals::default()
        })
    })
    .unwrap();
    assert_eq!(seen, vec![0, 0, 1, 0, 0]);
    assert_eq!(run.trades.entry_bar, vec![2]);
    assert_eq!(run.trades.exit_bar, vec![3]);
}

#[test]
fn callback_signals_are_validated_even_on_suppressed_bars() {
    let lookup = |_: &str| None;
    let inputs = CandleInputs {
        time_us: &[0],
        open: None,
        high: None,
        low: None,
        close: Some(&[100.]),
        signals: SignalColumns {
            entry: &[0],
            exit_long: &[false],
            exit_short: &[false],
            strength: &[0.],
            bar_index: None,
        },
        volatility: None,
        tradable: Some(&[false]),
        columns: &lookup,
        protective: None,
        intrabar: None,
    };
    let config = CandleConfig {
        initial_capital: 10000.,
        point_value: 1.,
        costs: None,
        sizing: Sizing::FixedQuantity {
            quantity: 1.,
            scale_by_strength: false,
        },
        holding_period_bars: None,
        exit_params: ExitParams::default(),
        day_trade: None,
        force_close_at_end: false,
    };
    for signals in [
        BarSignals {
            entry: 2,
            ..BarSignals::default()
        },
        BarSignals {
            strength: f64::NAN,
            ..BarSignals::default()
        },
    ] {
        let result =
            run_candle_with_callback(&inputs, &config, |_, _| Ok::<_, CandleError>(signals));
        assert!(matches!(
            result,
            Err(CandleError::InvalidSignal { bar: 0, .. })
        ));
    }
}
