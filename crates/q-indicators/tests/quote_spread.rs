#![forbid(unsafe_code)]

use q_indicators::quote_spread_bars;
use q_parity::determinism::check_double_run_with;

const TOL: f64 = 1e-12;

fn assert_close(a: f64, e: f64, ctx: &str) {
    if e.is_nan() {
        assert!(a.is_nan(), "{ctx}: expected NaN, got {a}");
        return;
    }
    assert!((a - e).abs() <= TOL, "{ctx}: got {a}, expected {e}");
}

#[test]
fn criterion_10_relative_spread_and_bar_aggregation() {
    let spread = (100.1 - 99.9) / ((100.1 + 99.9) / 2.0);
    assert_close(spread, 0.002, "hand spread");

    let time = [0i64];
    let bid = [99.9];
    let ask = [100.1];
    let flags = [6i64];
    let session = [1i64];
    let open = [0i64];
    let close = [60_000i64];
    let out =
        quote_spread_bars(&time, &bid, &ask, &flags, &session, &open, &close, 60_000).unwrap();
    assert_close(out.last[0], 0.002, "last");
    assert_close(out.time_weighted_mean[0], 0.002, "mean single quote");
    assert_close(out.max[0], 0.002, "max");
    assert_close(out.coverage[0], 1.0, "coverage full bar");

    let time2 = [0i64, 10_000i64];
    let bid2 = [99.9, 99.8];
    let ask2 = [100.1, 100.2];
    let flags2 = [6i64, 6i64];
    let session2 = [1i64, 1i64];
    let s10 = (100.1 - 99.9) / 100.0;
    let s20 = (100.2 - 99.8) / 100.0;
    #[expect(
        clippy::suboptimal_flops,
        reason = "hand-computed fixture mean must not use fused multiply-add"
    )]
    let expected_mean = (10_000.0 * s10 + 50_000.0 * s20) / 60_000.0;
    let out2 = quote_spread_bars(
        &time2, &bid2, &ask2, &flags2, &session2, &open, &close, 120_000,
    )
    .unwrap();
    assert_close(out2.time_weighted_mean[0], expected_mean, "weighted mean");
    assert_close(out2.max[0], s20, "max second spread");
    assert_close(out2.coverage[0], 1.0, "full coverage");
}

#[test]
fn criterion_11_quote_rules() {
    let open = [0i64];
    let close = [60_000i64];
    let session = [1i64];

    let time = [0i64];
    let bid = [99.9];
    let ask = [100.1];
    let flags = [6i64];
    let out =
        quote_spread_bars(&time, &bid, &ask, &flags, &session, &open, &close, 30_000).unwrap();
    assert_close(out.coverage[0], 0.5, "expiry halves coverage");

    let flags_trade = [0i64];
    let out_trade = quote_spread_bars(
        &time,
        &bid,
        &ask,
        &flags_trade,
        &session,
        &open,
        &close,
        120_000,
    )
    .unwrap();
    assert_close(out_trade.coverage[0], 0.0, "trade-only no quote");
    assert!(out_trade.time_weighted_mean[0].is_nan());

    let bid_cross = [100.1];
    let ask_cross = [99.9];
    let out_cross = quote_spread_bars(
        &time, &bid_cross, &ask_cross, &flags, &session, &open, &close, 120_000,
    )
    .unwrap();
    assert_close(out_cross.coverage[0], 0.0, "crossed quote");

    let time_sess = [0i64, 10_000i64];
    let session_chg = [1i64, 2i64];
    let bid2 = [99.9, 99.9];
    let ask2 = [100.1, 100.1];
    let flags_sess = [6i64, 0i64];
    let out_sess = quote_spread_bars(
        &time_sess,
        &bid2,
        &ask2,
        &flags_sess,
        &session_chg,
        &open,
        &close,
        120_000,
    )
    .unwrap();
    assert_close(
        out_sess.coverage[0],
        10_000.0 / 60_000.0,
        "session change ends quote",
    );

    let open2 = [0i64, 60_000i64];
    let close2 = [60_000i64, 120_000i64];
    let time_at_close = [60_000i64];
    let out_boundary = quote_spread_bars(
        &time_at_close,
        &bid,
        &ask,
        &flags,
        &session,
        &open2,
        &close2,
        120_000,
    )
    .unwrap();
    assert_close(
        out_boundary.coverage[0],
        0.0,
        "tick at close not in first bar",
    );
    assert_close(out_boundary.coverage[1], 1.0, "tick at close in second bar");

    let time_two = [0i64, 10_000i64];
    let bid_two = [99.9, 99.8];
    let ask_two = [100.1, 100.2];
    let flags_two = [6i64, 6i64];
    let session_two = [1i64, 1i64];
    let _ = check_double_run_with(|| {
        quote_spread_bars(
            &time_two,
            &bid_two,
            &ask_two,
            &flags_two,
            &session_two,
            &open,
            &close,
            120_000,
        )
        .unwrap()
        .time_weighted_mean
    });
}
