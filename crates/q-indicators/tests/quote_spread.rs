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

const BID: [f64; 1] = [99.9];
const ASK: [f64; 1] = [100.1];
const QUOTE: [i64; 1] = [6];
const SESSION: [i64; 1] = [1];

#[test]
fn tick_before_the_bar_carries_in_without_adding_duration() {
    // Quote 30 s before a 60 s bar, alive for 120 s: it covers the whole bar and no more.
    let out = quote_spread_bars(
        &[-30_000],
        &BID,
        &ASK,
        &QUOTE,
        &SESSION,
        &[0],
        &[60_000],
        120_000,
    )
    .unwrap();
    assert_close(out.coverage[0], 1.0, "carry-in coverage");
    assert_close(out.time_weighted_mean[0], 0.002, "carry-in mean");
    assert_close(out.last[0], 0.002, "carry-in last");

    // Alive for 50 s only: 20 s remain inside the bar and nothing is carried at the close.
    let out = quote_spread_bars(
        &[-30_000],
        &BID,
        &ASK,
        &QUOTE,
        &SESSION,
        &[0],
        &[60_000],
        50_000,
    )
    .unwrap();
    assert_close(out.coverage[0], 20_000.0 / 60_000.0, "partial carry-in");
    assert!(out.last[0].is_nan(), "expired before the close");
}

#[test]
fn quote_expired_before_the_bar_gives_no_value() {
    let out = quote_spread_bars(
        &[-10_800_000],
        &BID,
        &ASK,
        &QUOTE,
        &SESSION,
        &[0],
        &[60_000],
        60_000,
    )
    .unwrap();
    assert_close(out.coverage[0], 0.0, "expired coverage");
    assert!(out.time_weighted_mean[0].is_nan());
    assert!(out.max[0].is_nan());
    assert!(out.last[0].is_nan());
}

#[test]
fn tick_in_a_gap_between_bars_counts_only_inside_the_next_bar() {
    let out = quote_spread_bars(
        &[90_000],
        &BID,
        &ASK,
        &QUOTE,
        &SESSION,
        &[0, 120_000],
        &[60_000, 180_000],
        600_000,
    )
    .unwrap();
    assert_close(out.coverage[0], 0.0, "first bar has no quote");
    assert_close(out.coverage[1], 1.0, "second bar fully covered");
}

#[test]
fn session_change_before_the_bar_ends_the_carried_quote() {
    // A trade-only tick of the next session arrives in the gap.
    let out = quote_spread_bars(
        &[0, 90_000],
        &[99.9, 99.9],
        &[100.1, 100.1],
        &[6, 0],
        &[1, 2],
        &[120_000],
        &[180_000],
        600_000,
    )
    .unwrap();
    assert_close(out.coverage[0], 0.0, "session change in the gap");
    assert!(out.last[0].is_nan());
}

#[test]
fn quote_expiry_at_and_before_the_close() {
    let out =
        quote_spread_bars(&[0], &BID, &ASK, &QUOTE, &SESSION, &[0], &[60_000], 60_000).unwrap();
    assert_close(out.coverage[0], 1.0, "covered until the close");
    assert_close(out.last[0], 0.002, "still readable at its expiry instant");

    // The same quote adds no duration to a bar that opens at its expiry.
    let next = quote_spread_bars(
        &[0],
        &BID,
        &ASK,
        &QUOTE,
        &SESSION,
        &[0, 60_000],
        &[60_000, 120_000],
        60_000,
    )
    .unwrap();
    assert_close(next.coverage[1], 0.0, "expired at the next open");
    assert!(next.last[1].is_nan());

    let empty = quote_spread_bars(
        &[0],
        &BID,
        &ASK,
        &QUOTE,
        &SESSION,
        &[90_000],
        &[90_000],
        60_000,
    )
    .unwrap();
    assert!(empty.last[0].is_nan(), "zero-length bar after expiry");
    assert_close(empty.coverage[0], 0.0, "zero-length bar");
}

#[test]
fn coverage_is_bounded_and_prefix_causal_on_gapped_bars() {
    // Deterministic pseudo-random ticks over two sessions of bars separated by a gap.
    let mut state = 0x2545_f491_4f6c_dd1d_u64;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let mut time = Vec::new();
    let mut bid = Vec::new();
    let mut ask = Vec::new();
    let mut flags = Vec::new();
    let mut session = Vec::new();
    let mut t = -120_000i64;
    while t < 1_400_000 {
        t += (next() % 25_000) as i64;
        let width = (next() % 40) as f64 / 100.0;
        time.push(t);
        bid.push(100.0 - width);
        ask.push(if next() % 17 == 0 {
            99.0
        } else {
            100.0 + width
        });
        flags.push(if next() % 5 == 0 { 0 } else { 6 });
        session.push(if t < 700_000 { 1 } else { 2 });
    }
    let mut open = Vec::new();
    let mut close = Vec::new();
    for start in [0i64, 900_000] {
        for k in 0..8 {
            open.push(start + k * 60_000);
            close.push(start + (k + 1) * 60_000);
        }
    }
    let run = |bars: usize| {
        quote_spread_bars(
            &time,
            &bid,
            &ask,
            &flags,
            &session,
            &open[..bars],
            &close[..bars],
            45_000,
        )
        .unwrap()
    };
    let full = run(open.len());
    assert!(full.coverage.iter().all(|c| (0.0..=1.0).contains(c)));
    assert!(full.coverage.iter().any(|c| *c > 0.0 && *c < 1.0));
    for bars in 1..open.len() {
        let prefix = run(bars);
        for i in 0..bars {
            assert_eq!(prefix.coverage[i].to_bits(), full.coverage[i].to_bits());
            assert_eq!(
                prefix.time_weighted_mean[i].to_bits(),
                full.time_weighted_mean[i].to_bits()
            );
            assert_eq!(prefix.max[i].to_bits(), full.max[i].to_bits());
            assert_eq!(prefix.last[i].to_bits(), full.last[i].to_bits());
        }
    }
    let _ = check_double_run_with(|| run(open.len()).time_weighted_mean);
}
