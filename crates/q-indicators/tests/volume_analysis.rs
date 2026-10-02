use q_indicators::volume::{
    batch_volume, AggressorSide, TradeInput, VolumeConfig, VolumeError, VolumeState, BUY_FLAG,
    SELL_FLAG,
};

fn trade(time_msc: i64, volume: f64, raw_flags: u32, session_key: u64) -> TradeInput {
    TradeInput {
        time_msc,
        price: 10.0,
        volume,
        raw_flags,
        session_key,
        bar_open_msc: 0,
        bar_close_msc: 60_000,
    }
}

fn assert_output_bitwise_eq(left: &q_indicators::VolumeOutput, right: &q_indicators::VolumeOutput) {
    assert_eq!(left.bar_open_msc, right.bar_open_msc);
    assert_eq!(left.bar_close_msc, right.bar_close_msc);
    assert_eq!(left.buy_volume.to_bits(), right.buy_volume.to_bits());
    assert_eq!(left.sell_volume.to_bits(), right.sell_volume.to_bits());
    assert_eq!(
        left.unknown_volume.to_bits(),
        right.unknown_volume.to_bits()
    );
    assert_eq!(left.total_volume.to_bits(), right.total_volume.to_bits());
    assert_eq!(left.delta.to_bits(), right.delta.to_bits());
    assert_eq!(
        left.cumulative_delta.to_bits(),
        right.cumulative_delta.to_bits()
    );
    assert_eq!(
        left.classified_share.map(f64::to_bits),
        right.classified_share.map(f64::to_bits)
    );
    assert_eq!(
        left.trade_rate.map(f64::to_bits),
        right.trade_rate.map(f64::to_bits)
    );
    assert_eq!(left.rate_warmed_up, right.rate_warmed_up);
    assert_eq!(left.large_prints.len(), right.large_prints.len());
    for (left, right) in left.large_prints.iter().zip(&right.large_prints) {
        assert_eq!(left.side, right.side);
        assert_eq!(left.time_msc, right.time_msc);
        assert_eq!(left.price.to_bits(), right.price.to_bits());
        assert_eq!(left.volume.to_bits(), right.volume.to_bits());
    }
}

#[test]
fn aggregates_classified_and_unknown_volume_and_resets_cvd_by_session() {
    let mut state = VolumeState::new(VolumeConfig::default()).unwrap();
    state.push(trade(1, 120.0, BUY_FLAG, 7)).unwrap();
    state.push(trade(2, 30.0, SELL_FLAG, 7)).unwrap();
    let output = state.push(trade(3, 50.0, BUY_FLAG | SELL_FLAG, 7)).unwrap();

    assert_eq!(output.buy_volume.to_bits(), 120.0_f64.to_bits());
    assert_eq!(output.sell_volume.to_bits(), 30.0_f64.to_bits());
    assert_eq!(output.unknown_volume.to_bits(), 50.0_f64.to_bits());
    assert_eq!(output.delta.to_bits(), 90.0_f64.to_bits());
    assert_eq!(output.cumulative_delta.to_bits(), 90.0_f64.to_bits());
    assert_eq!(output.total_volume.to_bits(), 200.0_f64.to_bits());
    assert_eq!(
        output.classified_share.map(f64::to_bits),
        Some(0.75_f64.to_bits())
    );

    let next = state.push(trade(4, 2.0, SELL_FLAG, 8)).unwrap();
    assert_eq!(next.cumulative_delta.to_bits(), (-2.0_f64).to_bits());
}

#[test]
fn both_or_neither_flags_are_unknown_and_equal_timestamps_keep_multiplicity() {
    assert_eq!(BUY_FLAG, 32);
    assert_eq!(SELL_FLAG, 64);
    let mut state = VolumeState::new(VolumeConfig::default()).unwrap();
    state.push(trade(1, 4.0, BUY_FLAG, 1)).unwrap();
    let first = state.push(trade(2, 3.0, 0, 1)).unwrap();
    let second = state.push(trade(2, 3.0, BUY_FLAG | SELL_FLAG, 1)).unwrap();

    assert_eq!(AggressorSide::from_flags(0), AggressorSide::Unknown);
    assert_eq!(
        AggressorSide::from_flags(BUY_FLAG | SELL_FLAG),
        AggressorSide::Unknown
    );
    assert_eq!(first.unknown_volume.to_bits(), 3.0_f64.to_bits());
    assert_eq!(second.unknown_volume.to_bits(), 6.0_f64.to_bits());
    assert_eq!(second.total_volume.to_bits(), 10.0_f64.to_bits());
    assert_eq!(second.delta.to_bits(), 4.0_f64.to_bits());
}

#[test]
fn rate_window_uses_open_left_closed_right_boundaries_and_ages_on_advance() {
    let config = VolumeConfig {
        window_ms: 1_000,
        ..VolumeConfig::default()
    };
    let mut state = VolumeState::new(config).unwrap();
    state.push(trade(0, 1.0, BUY_FLAG, 1)).unwrap();
    state.push(trade(1_000, 1.0, BUY_FLAG, 1)).unwrap();
    let at_boundary = state.advance(1_000).unwrap();
    assert_eq!(at_boundary.trade_rate, Some(1.0));
    assert!(at_boundary.rate_warmed_up);

    let after_boundary = state.advance(2_000).unwrap();
    assert_eq!(after_boundary.trade_rate, Some(0.0));
    assert!(after_boundary.rate_warmed_up);
}

#[test]
fn large_print_threshold_is_inclusive() {
    let config = VolumeConfig {
        large_print_threshold: 100.0,
        ..VolumeConfig::default()
    };
    let mut state = VolumeState::new(config).unwrap();
    let output = state.push(trade(10, 100.0, SELL_FLAG, 1)).unwrap();
    assert_eq!(output.large_prints.len(), 1);
    assert_eq!(output.large_prints[0].side, AggressorSide::Sell);
    assert_eq!(output.large_prints[0].time_msc, 10);
    assert_eq!(output.large_prints[0].price.to_bits(), 10.0_f64.to_bits());
    assert_eq!(output.large_prints[0].volume.to_bits(), 100.0_f64.to_bits());
}

#[test]
fn batch_and_incremental_outputs_match_at_every_trade_prefix_and_replay() {
    let trades = [
        trade(0, 120.0, BUY_FLAG, 1),
        trade(500, 30.0, SELL_FLAG, 1),
        trade(500, 50.0, 0, 1),
        trade(1_000, 100.0, BUY_FLAG | SELL_FLAG, 1),
    ];
    let config = VolumeConfig {
        window_ms: 1_000,
        large_print_threshold: 50.0,
    };
    let batch = batch_volume(config, &trades, &[1_000, 2_000]).unwrap();
    let replay = batch_volume(config, &trades, &[1_000, 2_000]).unwrap();
    for (left, right) in batch.iter().zip(&replay) {
        assert_output_bitwise_eq(left, right);
    }

    let mut state = VolumeState::new(config).unwrap();
    for (index, input) in trades.iter().enumerate() {
        let incremental = state.push(*input).unwrap();
        assert_output_bitwise_eq(&batch[index], &incremental);
    }
    assert_output_bitwise_eq(&batch[trades.len()], &state.advance(1_000).unwrap());
    assert_output_bitwise_eq(&batch[trades.len() + 1], &state.advance(2_000).unwrap());
}

#[test]
fn unknown_only_volume_has_zero_classified_delta_and_zero_share() {
    let mut state = VolumeState::new(VolumeConfig::default()).unwrap();
    let output = state.push(trade(1, 2.0, 0, 1)).unwrap();
    assert_eq!(output.delta.to_bits(), 0.0_f64.to_bits());
    assert_eq!(output.cumulative_delta.to_bits(), 0.0_f64.to_bits());
    assert_eq!(
        output.classified_share.map(f64::to_bits),
        Some(0.0_f64.to_bits())
    );
}

#[test]
fn rejected_inputs_preserve_the_previous_output() {
    let mut state = VolumeState::new(VolumeConfig::default()).unwrap();
    state.push(trade(10, 2.0, BUY_FLAG, 1)).unwrap();
    let before = state.current();

    let mut invalid = trade(11, 0.0, SELL_FLAG, 1);
    assert!(matches!(
        state.push(invalid),
        Err(VolumeError::InvalidTrade { field: "volume" })
    ));
    assert_eq!(state.current(), before);

    invalid = trade(9, 3.0, SELL_FLAG, 1);
    assert!(matches!(
        state.push(invalid),
        Err(VolumeError::DecreasingTime { .. })
    ));
    assert_eq!(state.current(), before);

    invalid = trade(11, 3.0, SELL_FLAG, 1);
    invalid.bar_close_msc = 11;
    assert!(matches!(
        state.push(invalid),
        Err(VolumeError::InvalidBarInterval)
    ));
    assert_eq!(state.current(), before);
}

#[test]
fn invalid_config_and_checkpoints_are_rejected() {
    assert!(VolumeState::new(VolumeConfig {
        window_ms: 999,
        ..VolumeConfig::default()
    })
    .is_err());
    assert!(VolumeState::new(VolumeConfig {
        large_print_threshold: f64::NAN,
        ..VolumeConfig::default()
    })
    .is_err());

    let trades = [trade(10, 1.0, BUY_FLAG, 1)];
    assert!(matches!(
        batch_volume(VolumeConfig::default(), &trades, &[9]),
        Err(VolumeError::CheckpointBeforeLastInput { .. })
    ));
}

#[test]
fn rate_window_capacity_overflow_preserves_state() {
    let mut state = VolumeState::new(VolumeConfig::default()).unwrap();
    for _ in 0..q_indicators::MAX_RATE_EVENTS {
        state.push(trade(10, 1.0, BUY_FLAG, 1)).unwrap();
    }
    let before = state.current();
    assert!(matches!(
        state.push(trade(10, 1.0, BUY_FLAG, 1)),
        Err(VolumeError::CapacityExceeded {
            capacity: q_indicators::MAX_RATE_EVENTS
        })
    ));
    assert_eq!(state.current(), before);
}

#[test]
fn arithmetic_overflow_does_not_mutate_accumulators() {
    let mut state = VolumeState::new(VolumeConfig::default()).unwrap();
    state.push(trade(10, f64::MAX, BUY_FLAG, 1)).unwrap();
    let before = state.current();
    assert_eq!(
        state.push(trade(11, f64::MAX, BUY_FLAG, 1)),
        Err(VolumeError::ArithmeticOverflow)
    );
    assert_eq!(state.current(), before);
}
