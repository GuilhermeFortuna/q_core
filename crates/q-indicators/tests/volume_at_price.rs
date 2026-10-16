#![forbid(unsafe_code)]

use q_indicators::{volume_at_price, volume_at_price_levels, IndicatorError, BUY_FLAG, SELL_FLAG};
use q_parity::determinism::check_double_run_with;

const T: f64 = 0.5;
const M: i64 = 1;
const VA: f64 = 0.7;

fn assert_close(a: f64, e: f64, ctx: &str) {
    const TOL: f64 = 1e-12;
    if e.is_nan() {
        assert!(a.is_nan(), "{ctx}: expected NaN, got {a}");
        return;
    }
    assert!(
        (a - e).abs() <= TOL,
        "{ctx}: got {a}, expected {e} (diff {})",
        (a - e).abs()
    );
}

fn both_flags() -> i64 {
    (BUY_FLAG | SELL_FLAG) as i64
}

#[test]
fn criterion_1_hand_computed_rows_and_poc() {
    let time = [0_i64, 1, 2, 3];
    let price = [100.0, 100.5, 100.0, 101.0];
    let volume = [10.0, 5.0, 20.0, 3.0];
    let flags = [
        BUY_FLAG as i64,
        BUY_FLAG as i64,
        SELL_FLAG as i64,
        both_flags(),
    ];
    let open = [0_i64];
    let close = [10_000_i64];
    let out = volume_at_price(&time, &price, &volume, &flags, &open, &close, T, M, VA).unwrap();
    assert_eq!(out.rows.segment, [0, 0, 0]);
    assert_close(out.rows.price[0], 100.0, "row 100");
    assert_close(out.rows.buy[0], 10.0, "buy 100");
    assert_close(out.rows.sell[0], 20.0, "sell 100");
    assert_close(out.rows.total[0], 30.0, "total 100");
    assert_close(out.rows.delta[0], -10.0, "delta 100");
    assert_close(out.rows.price[1], 100.5, "row 100.5");
    assert_close(out.rows.total[1], 5.0, "total 100.5");
    assert_close(out.rows.price[2], 101.0, "row 101");
    assert_close(out.rows.unknown[2], 3.0, "unknown 101");
    assert_close(out.segments.total[0], 38.0, "segment total");
    assert_close(out.segments.poc[0], 100.0, "poc");
}

#[test]
fn criterion_2_non_trades_and_skipped() {
    let time = [0_i64, 1, 2, 3, 4];
    let price = [100.0, 0.0, f64::NAN, 100.0, 100.0];
    let volume = [10.0, 5.0, 7.0, 0.0, 10.0];
    let flags = [
        0_i64,
        BUY_FLAG as i64,
        BUY_FLAG as i64,
        BUY_FLAG as i64,
        BUY_FLAG as i64,
    ];
    let open = [0_i64];
    let close = [100_i64];
    let out = volume_at_price(&time, &price, &volume, &flags, &open, &close, T, M, VA).unwrap();
    assert_eq!(out.rows.price.len(), 1);
    assert_close(out.rows.total[0], 10.0, "only valid trade");
    assert_eq!(out.segments.skipped_trades[0], 3);
    assert_eq!(out.segments.trades[0], 1);
}

#[test]
fn criterion_3_row_ticks_and_off_grid() {
    let time = [0_i64, 1, 2, 3];
    let price = [100.0, 100.5, 101.0, 100.2];
    let volume = [1.0, 1.0, 1.0, 1.0];
    let flags = [BUY_FLAG as i64; 4];
    let open = [0_i64];
    let close = [100_i64];
    let out = volume_at_price(&time, &price, &volume, &flags, &open, &close, T, 2, VA).unwrap();
    assert_eq!(out.rows.price.len(), 2);
    assert_close(out.rows.price[0], 100.0, "merged row");
    assert_close(out.rows.total[0], 3.0, "100+100.5+100.2");
    assert_close(out.rows.price[1], 101.0, "next row");
    assert_eq!(out.segments.off_grid_trades[0], 1);
}

#[test]
fn criterion_4_poc_tie_break() {
    let mk = |totals: &[f64]| {
        let n = totals.len();
        let time: Vec<i64> = (0..n as i64).collect();
        let price: Vec<f64> = totals
            .iter()
            .enumerate()
            .map(|(i, _)| {
                let step = i as f64 * T;
                100.0 + step
            })
            .collect();
        let volume = totals.to_vec();
        let flags = vec![BUY_FLAG as i64; n];
        volume_at_price(&time, &price, &volume, &flags, &[0], &[10_000], T, M, VA).unwrap()
    };
    let a = mk(&[10.0, 2.0, 10.0, 2.0, 2.0]);
    assert_close(a.segments.poc[0], 101.0, "centre tie third row");
    let b = mk(&[10.0, 1.0, 10.0]);
    assert_close(b.segments.poc[0], 100.0, "equal distance lower row");
}

#[test]
fn criterion_5_value_area_expansion() {
    let totals = [5.0, 10.0, 40.0, 10.0, 30.0, 5.0];
    let n = totals.len();
    let time: Vec<i64> = (0..n as i64).collect();
    let price: Vec<f64> = totals
        .iter()
        .enumerate()
        .map(|(i, _)| {
            let step = i as f64 * T;
            100.0 + step
        })
        .collect();
    let volume = totals.to_vec();
    let flags = vec![BUY_FLAG as i64; n];
    let out = volume_at_price(&time, &price, &volume, &flags, &[0], &[10_000], T, M, 0.7).unwrap();
    assert_close(out.segments.val[0], 100.5, "val second row");
    assert_close(out.segments.vah[0], 102.0, "vah fifth row");

    let full = volume_at_price(&time, &price, &volume, &flags, &[0], &[10_000], T, M, 1.0).unwrap();
    assert_close(full.segments.val[0], 100.0, "va=1 val");
    assert_close(full.segments.vah[0], 102.5, "va=1 vah");

    // Gap row inside range: volumes at rows 0, 2, 4 only.
    let gap_time = [0_i64, 1, 2];
    let gap_price = [100.0, 101.0, 102.0];
    let gap_vol = [10.0, 40.0, 10.0];
    let gap_flags = [BUY_FLAG as i64; 3];
    let gap = volume_at_price(
        &gap_time,
        &gap_price,
        &gap_vol,
        &gap_flags,
        &[0],
        &[10_000],
        T,
        M,
        0.7,
    )
    .unwrap();
    assert_close(gap.segments.poc[0], 101.0, "gap poc");
    assert_close(gap.segments.val[0], 100.0, "gap val");
    assert_close(gap.segments.vah[0], 102.0, "gap vah");
}

#[test]
fn criterion_6_segment_boundaries() {
    let time = [50_i64, 100, 150, 200];
    let price = [100.0, 100.0, 100.0, 100.0];
    let volume = [1.0, 2.0, 3.0, 4.0];
    let flags = [BUY_FLAG as i64; 4];
    let open = [0_i64, 100, 200];
    let close = [100_i64, 200, 300];
    let out = volume_at_price(&time, &price, &volume, &flags, &open, &close, T, M, VA).unwrap();
    assert_eq!(out.segments.trades, [1, 2, 1]);
    assert_close(out.segments.total[0], 1.0, "seg0 total");
    assert_close(out.segments.total[1], 5.0, "seg1 total");
    assert_close(out.segments.total[2], 4.0, "seg2 total");

    let outside = volume_at_price(
        &[500_i64],
        &[100.0],
        &[1.0],
        &[BUY_FLAG as i64],
        &[0],
        &[100],
        T,
        M,
        VA,
    )
    .unwrap();
    assert!(outside.segments.poc[0].is_nan());
    assert_eq!(outside.segments.trades[0], 0);

    let empty = volume_at_price(&[], &[], &[], &[], &[0], &[0], T, M, VA);
    assert!(matches!(
        empty,
        Err(IndicatorError::InvalidParameter { .. })
    ));
}

#[test]
fn criterion_7_volume_at_price_levels() {
    let time = [10_i64, 20, 30, 40];
    let price = [100.0, 100.5, 100.0, 101.0];
    let volume = [10.0, 5.0, 20.0, 3.0];
    let flags = [
        BUY_FLAG as i64,
        BUY_FLAG as i64,
        SELL_FLAG as i64,
        both_flags(),
    ];
    let session = [1_i64, 1, 1, 1];
    let seg = volume_at_price(&time, &price, &volume, &flags, &[0], &[100], T, M, VA).unwrap();

    let cp_time = [5_i64, 25, 50];
    let cp_sess = [1_i64, 1, 1];
    let lv = volume_at_price_levels(
        &time, &price, &volume, &flags, &session, &cp_time, &cp_sess, T, M, VA,
    )
    .unwrap();
    assert!(lv.poc[0].is_nan());
    assert_close(lv.total[0], 0.0, "before first trade");
    assert_close(lv.total[1], 15.0, "checkpoint before last trade");
    assert_close(lv.poc[2], seg.segments.poc[0], "final checkpoint poc");
    assert_close(lv.vah[2], seg.segments.vah[0], "final checkpoint vah");
    assert_close(lv.val[2], seg.segments.val[0], "final checkpoint val");
    assert_close(lv.total[2], seg.segments.total[0], "final total");

    let at_trade = volume_at_price_levels(
        &time,
        &price,
        &volume,
        &flags,
        &session,
        &[30_i64],
        &[1],
        T,
        M,
        VA,
    )
    .unwrap();
    assert_close(at_trade.total[0], 15.0, "strictly before trade at 30");

    let t2 = [10_i64, 20, 30, 40];
    let p2 = [100.0, 100.0, 100.0, 100.0];
    let v2 = [1.0, 2.0, 3.0, 4.0];
    let f2 = [BUY_FLAG as i64; 4];
    let s2 = [1_i64, 1, 2, 2];
    let lv2 =
        volume_at_price_levels(&t2, &p2, &v2, &f2, &s2, &[25, 35], &[1, 2], T, M, VA).unwrap();
    assert_close(lv2.total[0], 3.0, "session 1 only");
    assert_close(lv2.total[1], 3.0, "session 2 before trade at 40");
}

#[test]
fn criterion_8_determinism_rejections_and_prefix() {
    let time = [0_i64, 1, 2];
    let price = [100.0, 100.5, 101.0];
    let volume = [1.0, 2.0, 3.0];
    let flags = [BUY_FLAG as i64; 3];
    let open = [0_i64, 50];
    let close = [50_i64, 100];
    let run = |ticks: usize, segs: usize| {
        volume_at_price(
            &time[..ticks],
            &price[..ticks],
            &volume[..ticks],
            &flags[..ticks],
            &open[..segs],
            &close[..segs],
            T,
            M,
            VA,
        )
        .unwrap()
    };
    let full = run(3, 2);
    for segs in 1..=2 {
        let prefix = run(3, segs);
        for i in 0..segs {
            assert_eq!(
                prefix.segments.total[i].to_bits(),
                full.segments.total[i].to_bits()
            );
        }
    }
    let more_ticks = volume_at_price(
        &[0_i64, 1, 2, 99],
        &[100.0, 100.5, 101.0, 100.0],
        &[1.0, 2.0, 3.0, 9.0],
        &[BUY_FLAG as i64; 4],
        &open,
        &close,
        T,
        M,
        VA,
    )
    .unwrap();
    assert_eq!(
        more_ticks.segments.total[0].to_bits(),
        full.segments.total[0].to_bits()
    );
    let _ = check_double_run_with(|| run(3, 2).segments.total.clone());

    assert!(matches!(
        volume_at_price(
            &[1, 0],
            &price[..2],
            &volume[..2],
            &flags[..2],
            &open,
            &close,
            T,
            M,
            VA
        ),
        Err(IndicatorError::InvalidParameter { .. })
    ));
    assert!(matches!(
        volume_at_price(
            &time,
            &price,
            &volume,
            &flags,
            &[0, 40],
            &[100, 80],
            T,
            M,
            VA
        ),
        Err(IndicatorError::InvalidParameter { .. })
    ));
    assert!(matches!(
        volume_at_price(&time, &price, &volume, &flags, &open, &close, T, 0, VA),
        Err(IndicatorError::InvalidParameter { .. })
    ));
    assert!(matches!(
        volume_at_price(&time, &price, &volume, &flags, &open, &close, 0.0, M, VA),
        Err(IndicatorError::InvalidInput { .. })
    ));
    assert!(matches!(
        volume_at_price(&time, &price, &volume, &flags, &open, &close, T, M, 0.0),
        Err(IndicatorError::InvalidInput { .. })
    ));

    let session = [1_i64; 3];
    assert!(matches!(
        volume_at_price_levels(
            &time,
            &price,
            &volume,
            &flags,
            &session,
            &[10, 5],
            &[1, 1],
            T,
            M,
            VA,
        ),
        Err(IndicatorError::InvalidParameter { .. })
    ));
}

#[test]
fn totals_follow_tape_order_across_rows_and_sides() {
    let out = volume_at_price(
        &[0, 1, 2],
        &[100.0, 101.0, 101.0],
        &[1e16, 1.0, 1.0],
        &[BUY_FLAG as i64; 3],
        &[0],
        &[3],
        T,
        1,
        VA,
    )
    .unwrap();
    assert_eq!(out.segments.buy[0].to_bits(), 1e16_f64.to_bits());
    assert_eq!(out.segments.total[0].to_bits(), 1e16_f64.to_bits());
    let mixed = volume_at_price(
        &[0, 1, 2],
        &[100.0; 3],
        &[1.0, 1e16, 1.0],
        &[BUY_FLAG as i64, SELL_FLAG as i64, BUY_FLAG as i64],
        &[0],
        &[3],
        T,
        1,
        VA,
    )
    .unwrap();
    assert_eq!(mixed.rows.total[0].to_bits(), 1e16_f64.to_bits());
    assert_eq!(mixed.segments.total[0].to_bits(), 1e16_f64.to_bits());
}

#[test]
fn full_value_area_includes_small_volume_at_range_edge() {
    let out = volume_at_price(
        &[0, 1],
        &[100.0, 101.0],
        &[1e16, 1.0],
        &[BUY_FLAG as i64; 2],
        &[0],
        &[2],
        T,
        1,
        1.0,
    )
    .unwrap();
    assert_close(out.segments.val[0], 100.0, "full value area low");
    assert_close(out.segments.vah[0], 101.0, "full value area high");
}

#[test]
fn large_representable_rows_do_not_overflow_centre() {
    let out = volume_at_price(
        &[0],
        &[3e18],
        &[1.0],
        &[BUY_FLAG as i64],
        &[0],
        &[1],
        T,
        1,
        VA,
    )
    .unwrap();
    assert_close(out.segments.poc[0], 3e18, "large row poc");
}

#[test]
fn unrepresentable_tick_indices_are_rejected_by_both_kernels() {
    for price in [1e20, 1e308] {
        assert!(matches!(
            volume_at_price(
                &[0],
                &[price],
                &[1.0],
                &[BUY_FLAG as i64],
                &[0],
                &[1],
                T,
                1,
                VA,
            ),
            Err(IndicatorError::InvalidInput {
                parameter: "price",
                ..
            })
        ));
        assert!(matches!(
            volume_at_price_levels(
                &[0],
                &[price],
                &[1.0],
                &[BUY_FLAG as i64],
                &[1],
                &[1],
                &[1],
                T,
                1,
                VA,
            ),
            Err(IndicatorError::InvalidInput {
                parameter: "price",
                ..
            })
        ));
    }
}

#[test]
fn levels_do_not_process_prices_after_the_final_checkpoint() {
    let out = volume_at_price_levels(
        &[0, 2],
        &[100.0, 1e308],
        &[1.0; 2],
        &[BUY_FLAG as i64; 2],
        &[1; 2],
        &[1],
        &[1],
        T,
        1,
        VA,
    )
    .unwrap();
    assert_close(out.poc[0], 100.0, "checkpoint ignores suffix");
}
