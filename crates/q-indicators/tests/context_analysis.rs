#![forbid(unsafe_code)]

use q_indicators::context::{
    batch_context, classify_momentum, classify_trend, classify_volatility, classify_vwap,
    ContextConfig, ContextInput, ContextOutput, ContextReadingStatus, MomentumZone, RsiChangeLabel,
    TrendCategory, VolatilityCategory, VwapCategory, VwapExtension, VwapPosition,
};
use q_indicators::{ContextState, SessionVwapOutput};
use q_parity::determinism::check_double_run_with;

fn assert_opt_f64_bits_eq(actual: Option<f64>, expected: Option<f64>, ctx: &str) {
    match (actual, expected) {
        (None, None) => {}
        (Some(a), Some(e)) => {
            if a.is_nan() && e.is_nan() {
                return;
            }
            assert_eq!(
                a.to_bits(),
                e.to_bits(),
                "{ctx}: float bits mismatch: got {a} ({:#x}), expected {e} ({:#x})",
                a.to_bits(),
                e.to_bits()
            );
        }
        _ => panic!("{ctx}: Option mismatch: actual {actual:?}, expected {expected:?}"),
    }
}

fn assert_f64_bits_eq(actual: f64, expected: f64, ctx: &str) {
    if actual.is_nan() && expected.is_nan() {
        return;
    }
    assert_eq!(
        actual.to_bits(),
        expected.to_bits(),
        "{ctx}: float bits mismatch: got {actual} ({:#x}), expected {expected} ({:#x})",
        actual.to_bits(),
        expected.to_bits()
    );
}

fn assert_context_outputs_bits_eq(actual: &ContextOutput, expected: &ContextOutput, ctx: &str) {
    // Trend
    assert_eq!(
        actual.trend.status, expected.trend.status,
        "{ctx}: trend.status mismatch"
    );
    assert_eq!(
        actual.trend.category, expected.trend.category,
        "{ctx}: trend.category mismatch"
    );
    assert_eq!(
        actual.trend.reason, expected.trend.reason,
        "{ctx}: trend.reason mismatch"
    );
    assert_f64_bits_eq(
        actual.trend.evidence.fast_ema,
        expected.trend.evidence.fast_ema,
        &format!("{ctx}: trend.fast_ema"),
    );
    assert_f64_bits_eq(
        actual.trend.evidence.slow_ema,
        expected.trend.evidence.slow_ema,
        &format!("{ctx}: trend.slow_ema"),
    );
    assert_opt_f64_bits_eq(
        actual.trend.evidence.fast_slope,
        expected.trend.evidence.fast_slope,
        &format!("{ctx}: trend.fast_slope"),
    );
    assert_opt_f64_bits_eq(
        actual.trend.evidence.slow_slope,
        expected.trend.evidence.slow_slope,
        &format!("{ctx}: trend.slow_slope"),
    );
    assert_f64_bits_eq(
        actual.trend.evidence.spread,
        expected.trend.evidence.spread,
        &format!("{ctx}: trend.spread"),
    );
    assert_opt_f64_bits_eq(
        actual.trend.evidence.normalized_spread,
        expected.trend.evidence.normalized_spread,
        &format!("{ctx}: trend.normalized_spread"),
    );

    // Momentum
    assert_eq!(
        actual.momentum.status, expected.momentum.status,
        "{ctx}: momentum.status mismatch"
    );
    assert_eq!(
        actual.momentum.category, expected.momentum.category,
        "{ctx}: momentum.category mismatch"
    );
    assert_eq!(
        actual.momentum.reason, expected.momentum.reason,
        "{ctx}: momentum.reason mismatch"
    );
    assert_f64_bits_eq(
        actual.momentum.evidence.rsi,
        expected.momentum.evidence.rsi,
        &format!("{ctx}: momentum.rsi"),
    );
    assert_f64_bits_eq(
        actual.momentum.evidence.rsi_lower,
        expected.momentum.evidence.rsi_lower,
        &format!("{ctx}: momentum.rsi_lower"),
    );
    assert_f64_bits_eq(
        actual.momentum.evidence.rsi_upper,
        expected.momentum.evidence.rsi_upper,
        &format!("{ctx}: momentum.rsi_upper"),
    );
    assert_opt_f64_bits_eq(
        actual.momentum.evidence.rsi_change,
        expected.momentum.evidence.rsi_change,
        &format!("{ctx}: momentum.rsi_change"),
    );
    assert_eq!(
        actual.momentum.evidence.change_label, expected.momentum.evidence.change_label,
        "{ctx}: momentum.change_label mismatch"
    );

    // Volatility
    assert_eq!(
        actual.volatility.status, expected.volatility.status,
        "{ctx}: volatility.status mismatch"
    );
    assert_eq!(
        actual.volatility.category, expected.volatility.category,
        "{ctx}: volatility.category mismatch"
    );
    assert_eq!(
        actual.volatility.reason, expected.volatility.reason,
        "{ctx}: volatility.reason mismatch"
    );
    assert_f64_bits_eq(
        actual.volatility.evidence.atr,
        expected.volatility.evidence.atr,
        &format!("{ctx}: volatility.atr"),
    );
    assert_opt_f64_bits_eq(
        actual.volatility.evidence.baseline_atr,
        expected.volatility.evidence.baseline_atr,
        &format!("{ctx}: volatility.baseline_atr"),
    );
    assert_opt_f64_bits_eq(
        actual.volatility.evidence.ratio,
        expected.volatility.evidence.ratio,
        &format!("{ctx}: volatility.ratio"),
    );
    assert_f64_bits_eq(
        actual.volatility.evidence.lower_threshold,
        expected.volatility.evidence.lower_threshold,
        &format!("{ctx}: volatility.lower_threshold"),
    );
    assert_f64_bits_eq(
        actual.volatility.evidence.upper_threshold,
        expected.volatility.evidence.upper_threshold,
        &format!("{ctx}: volatility.upper_threshold"),
    );

    // VWAP
    assert_eq!(
        actual.vwap.status, expected.vwap.status,
        "{ctx}: vwap.status mismatch"
    );
    assert_eq!(
        actual.vwap.category, expected.vwap.category,
        "{ctx}: vwap.category mismatch"
    );
    assert_eq!(
        actual.vwap.reason, expected.vwap.reason,
        "{ctx}: vwap.reason mismatch"
    );
    assert_opt_f64_bits_eq(
        actual.vwap.evidence.vwap,
        expected.vwap.evidence.vwap,
        &format!("{ctx}: vwap.vwap"),
    );
    assert_opt_f64_bits_eq(
        actual.vwap.evidence.distance,
        expected.vwap.evidence.distance,
        &format!("{ctx}: vwap.distance"),
    );
    assert_f64_bits_eq(
        actual.vwap.evidence.extension_threshold,
        expected.vwap.evidence.extension_threshold,
        &format!("{ctx}: vwap.extension_threshold"),
    );
    assert_opt_f64_bits_eq(
        actual.vwap.evidence.std_dev,
        expected.vwap.evidence.std_dev,
        &format!("{ctx}: vwap.std_dev"),
    );
}

// -------------------------------------------------------------------------
// 1. Hand-computed fixtures covering every category and threshold boundary
// -------------------------------------------------------------------------

#[test]
fn test_trend_classification_categories_and_evidence() {
    let atr = 2.0;

    // Upward: fast > slow and both one-bar slopes > 0
    let up = classify_trend(105.0, 100.0, Some(104.0), Some(99.0), atr);
    assert_eq!(up.status, ContextReadingStatus::Available);
    assert_eq!(up.category, Some(TrendCategory::Upward));
    assert_eq!(up.reason, None);
    assert_f64_bits_eq(up.evidence.fast_ema, 105.0, "up fast_ema");
    assert_f64_bits_eq(up.evidence.slow_ema, 100.0, "up slow_ema");
    assert_opt_f64_bits_eq(up.evidence.fast_slope, Some(1.0), "up fast_slope");
    assert_opt_f64_bits_eq(up.evidence.slow_slope, Some(1.0), "up slow_slope");
    assert_f64_bits_eq(up.evidence.spread, 5.0, "up spread");
    assert_opt_f64_bits_eq(
        up.evidence.normalized_spread,
        Some(2.5),
        "up normalized_spread",
    );

    // Downward: fast < slow and both one-bar slopes < 0
    let down = classify_trend(95.0, 100.0, Some(96.0), Some(101.0), atr);
    assert_eq!(down.status, ContextReadingStatus::Available);
    assert_eq!(down.category, Some(TrendCategory::Downward));
    assert_eq!(down.reason, None);
    assert_opt_f64_bits_eq(down.evidence.fast_slope, Some(-1.0), "down fast_slope");
    assert_opt_f64_bits_eq(down.evidence.slow_slope, Some(-1.0), "down slow_slope");
    assert_f64_bits_eq(down.evidence.spread, -5.0, "down spread");
    assert_opt_f64_bits_eq(
        down.evidence.normalized_spread,
        Some(-2.5),
        "down normalized_spread",
    );

    // Balanced: fast == slow and both slopes == 0
    let bal = classify_trend(100.0, 100.0, Some(100.0), Some(100.0), atr);
    assert_eq!(bal.status, ContextReadingStatus::Available);
    assert_eq!(bal.category, Some(TrendCategory::Balanced));
    assert_eq!(bal.reason, None);
    assert_opt_f64_bits_eq(bal.evidence.fast_slope, Some(0.0), "bal fast_slope");
    assert_opt_f64_bits_eq(bal.evidence.slow_slope, Some(0.0), "bal slow_slope");
    assert_f64_bits_eq(bal.evidence.spread, 0.0, "bal spread");
    assert_opt_f64_bits_eq(
        bal.evidence.normalized_spread,
        Some(0.0),
        "bal normalized_spread",
    );

    // Mixed case 1: fast > slow but fast_slope < 0
    let mix1 = classify_trend(105.0, 100.0, Some(106.0), Some(99.0), atr);
    assert_eq!(mix1.status, ContextReadingStatus::Available);
    assert_eq!(mix1.category, Some(TrendCategory::Mixed));

    // Mixed case 2: fast > slow but slow_slope < 0
    let mix2 = classify_trend(105.0, 100.0, Some(104.0), Some(101.0), atr);
    assert_eq!(mix2.status, ContextReadingStatus::Available);
    assert_eq!(mix2.category, Some(TrendCategory::Mixed));

    // Mixed case 3: fast < slow but fast_slope > 0
    let mix3 = classify_trend(95.0, 100.0, Some(94.0), Some(101.0), atr);
    assert_eq!(mix3.status, ContextReadingStatus::Available);
    assert_eq!(mix3.category, Some(TrendCategory::Mixed));

    // Mixed case 4: fast == slow but slopes are non-zero
    let mix4 = classify_trend(100.0, 100.0, Some(99.0), Some(99.0), atr);
    assert_eq!(mix4.status, ContextReadingStatus::Available);
    assert_eq!(mix4.category, Some(TrendCategory::Mixed));

    // Warming up: missing prior bar for slope
    let warm = classify_trend(100.0, 100.0, None, None, atr);
    assert_eq!(warm.status, ContextReadingStatus::WarmingUp);
    assert_eq!(warm.category, None);
    assert_eq!(warm.reason, Some("prior bar required for slope"));
    assert_opt_f64_bits_eq(warm.evidence.fast_slope, None, "warm fast_slope");
    assert_opt_f64_bits_eq(warm.evidence.slow_slope, None, "warm slow_slope");
    assert_f64_bits_eq(warm.evidence.spread, 0.0, "warm spread");
    assert_opt_f64_bits_eq(
        warm.evidence.normalized_spread,
        Some(0.0),
        "warm normalized_spread",
    );

    // Absent ATR removes only normalized evidence
    let no_atr = classify_trend(105.0, 100.0, Some(104.0), Some(99.0), f64::NAN);
    assert_eq!(no_atr.status, ContextReadingStatus::Available);
    assert_eq!(no_atr.category, Some(TrendCategory::Upward));
    assert_opt_f64_bits_eq(
        no_atr.evidence.normalized_spread,
        None,
        "no_atr normalized_spread",
    );
    assert_f64_bits_eq(no_atr.evidence.spread, 5.0, "no_atr spread");

    // Zero ATR removes only normalized evidence
    let zero_atr = classify_trend(105.0, 100.0, Some(104.0), Some(99.0), 0.0);
    assert_eq!(zero_atr.status, ContextReadingStatus::Available);
    assert_eq!(zero_atr.category, Some(TrendCategory::Upward));
    assert_opt_f64_bits_eq(
        zero_atr.evidence.normalized_spread,
        None,
        "zero_atr normalized_spread",
    );
}

#[test]
fn test_momentum_classification_threshold_boundaries() {
    let config = ContextConfig::default(); // rsi_lower: 30.0, rsi_upper: 70.0

    // RSI < 30.0 -> LowerZone
    let m_low = classify_momentum(29.9999, Some(28.0), &config);
    assert_eq!(m_low.status, ContextReadingStatus::Available);
    assert_eq!(m_low.category, Some(MomentumZone::LowerZone));
    assert_eq!(m_low.evidence.change_label, Some(RsiChangeLabel::Rising));

    // RSI == 30.0 -> MiddleZone (threshold equality is middle)
    let m_eq_low = classify_momentum(30.0, Some(30.0), &config);
    assert_eq!(m_eq_low.status, ContextReadingStatus::Available);
    assert_eq!(m_eq_low.category, Some(MomentumZone::MiddleZone));
    assert_eq!(
        m_eq_low.evidence.change_label,
        Some(RsiChangeLabel::Unchanged)
    );

    // RSI between 30 and 70 -> MiddleZone
    let m_mid = classify_momentum(50.0, Some(55.0), &config);
    assert_eq!(m_mid.status, ContextReadingStatus::Available);
    assert_eq!(m_mid.category, Some(MomentumZone::MiddleZone));
    assert_eq!(m_mid.evidence.change_label, Some(RsiChangeLabel::Falling));
    assert_opt_f64_bits_eq(m_mid.evidence.rsi_change, Some(-5.0), "m_mid rsi_change");

    // RSI == 70.0 -> MiddleZone (threshold equality is middle)
    let m_eq_high = classify_momentum(70.0, Some(69.0), &config);
    assert_eq!(m_eq_high.status, ContextReadingStatus::Available);
    assert_eq!(m_eq_high.category, Some(MomentumZone::MiddleZone));

    // RSI > 70.0 -> UpperZone
    let m_high = classify_momentum(70.0001, Some(69.0), &config);
    assert_eq!(m_high.status, ContextReadingStatus::Available);
    assert_eq!(m_high.category, Some(MomentumZone::UpperZone));

    // Warming up (NaN RSI)
    let m_warm = classify_momentum(f64::NAN, None, &config);
    assert_eq!(m_warm.status, ContextReadingStatus::WarmingUp);
    assert_eq!(m_warm.category, None);
    assert_eq!(m_warm.reason, Some("rsi warming up"));
    assert!(m_warm.evidence.rsi.is_nan());
    assert_eq!(m_warm.evidence.rsi_change, None);
    assert_eq!(m_warm.evidence.change_label, None);

    // Missing preceding RSI leaves change unavailable
    let m_no_prev = classify_momentum(45.0, None, &config);
    assert_eq!(m_no_prev.status, ContextReadingStatus::Available);
    assert_eq!(m_no_prev.category, Some(MomentumZone::MiddleZone));
    assert_eq!(m_no_prev.evidence.rsi_change, None);
    assert_eq!(m_no_prev.evidence.change_label, None);
}

#[test]
fn test_volatility_classification_threshold_boundaries_and_zero_baseline() {
    let config = ContextConfig::default(); // lower: 0.8, upper: 1.2
    let baseline = 10.0;

    // ratio < 0.8 -> Contracting
    let v_low = classify_volatility(7.999, baseline, &config);
    assert_eq!(v_low.status, ContextReadingStatus::Available);
    assert_eq!(v_low.category, Some(VolatilityCategory::Contracting));
    assert_opt_f64_bits_eq(v_low.evidence.ratio, Some(7.999 / baseline), "v_low ratio");

    // ratio == 0.8 -> Typical (threshold equality is typical)
    let v_eq_low = classify_volatility(8.0, baseline, &config);
    assert_eq!(v_eq_low.status, ContextReadingStatus::Available);
    assert_eq!(v_eq_low.category, Some(VolatilityCategory::Typical));
    assert_opt_f64_bits_eq(v_eq_low.evidence.ratio, Some(0.8), "v_eq_low ratio");

    // ratio between 0.8 and 1.2 -> Typical
    let v_mid = classify_volatility(10.0, baseline, &config);
    assert_eq!(v_mid.status, ContextReadingStatus::Available);
    assert_eq!(v_mid.category, Some(VolatilityCategory::Typical));
    assert_opt_f64_bits_eq(v_mid.evidence.ratio, Some(1.0), "v_mid ratio");

    // ratio == 1.2 -> Typical (threshold equality is typical)
    let v_eq_high = classify_volatility(12.0, baseline, &config);
    assert_eq!(v_eq_high.status, ContextReadingStatus::Available);
    assert_eq!(v_eq_high.category, Some(VolatilityCategory::Typical));
    assert_opt_f64_bits_eq(v_eq_high.evidence.ratio, Some(1.2), "v_eq_high ratio");

    // ratio > 1.2 -> Expanding
    let v_high = classify_volatility(12.001, baseline, &config);
    assert_eq!(v_high.status, ContextReadingStatus::Available);
    assert_eq!(v_high.category, Some(VolatilityCategory::Expanding));
    assert_opt_f64_bits_eq(
        v_high.evidence.ratio,
        Some(12.001 / baseline),
        "v_high ratio",
    );

    // Zero baseline denominator: must be rejected as Unavailable, never infinite
    let v_zero_denom = classify_volatility(0.0, 0.0, &config);
    assert_eq!(v_zero_denom.status, ContextReadingStatus::Unavailable);
    assert_eq!(v_zero_denom.category, None);
    assert_eq!(
        v_zero_denom.reason,
        Some("nonpositive or nonfinite atr baseline")
    );
    assert_eq!(v_zero_denom.evidence.ratio, None);

    // Negative baseline denominator
    let v_neg_denom = classify_volatility(5.0, -1.0, &config);
    assert_eq!(v_neg_denom.status, ContextReadingStatus::Unavailable);
    assert_eq!(v_neg_denom.category, None);
    assert_eq!(
        v_neg_denom.reason,
        Some("nonpositive or nonfinite atr baseline")
    );
    assert_eq!(v_neg_denom.evidence.ratio, None);

    // Warmup (NaN baseline or NaN ATR)
    let v_warm = classify_volatility(f64::NAN, 10.0, &config);
    assert_eq!(v_warm.status, ContextReadingStatus::WarmingUp);
    assert_eq!(v_warm.category, None);
    assert_eq!(v_warm.reason, Some("volatility baseline warming up"));
}

#[test]
fn test_vwap_classification_threshold_boundaries_and_rejection() {
    let config = ContextConfig::default(); // extension: 1.0 ATR
    let atr = 2.0;
    let vwap_val = 100.0;
    let vwap_out = Some(SessionVwapOutput {
        vwap: vwap_val,
        std_dev: 1.5,
    });

    // Above and Extended: close = 102.0, distance = (102 - 100) / 2 = 1.0 (>= 1.0 threshold)
    let vw_eq_ext = classify_vwap(102.0, atr, vwap_out, &config, None);
    assert_eq!(vw_eq_ext.status, ContextReadingStatus::Available);
    assert_eq!(
        vw_eq_ext.category,
        Some(VwapCategory {
            position: VwapPosition::Above,
            extension: Some(VwapExtension::Extended),
        })
    );
    assert_opt_f64_bits_eq(vw_eq_ext.evidence.distance, Some(1.0), "vw_eq_ext distance");

    // Above and Near: close = 101.9, distance = 0.95 (< 1.0)
    let vw_near = classify_vwap(101.9, atr, vwap_out, &config, None);
    assert_eq!(vw_near.status, ContextReadingStatus::Available);
    assert_eq!(
        vw_near.category,
        Some(VwapCategory {
            position: VwapPosition::Above,
            extension: Some(VwapExtension::Near),
        })
    );
    assert_opt_f64_bits_eq(
        vw_near.evidence.distance,
        Some((101.9 - 100.0) / atr),
        "vw_near distance",
    );

    // At VWAP: close == 100.0, distance = 0.0 (< 1.0 -> Near)
    let vw_at = classify_vwap(100.0, atr, vwap_out, &config, None);
    assert_eq!(vw_at.status, ContextReadingStatus::Available);
    assert_eq!(
        vw_at.category,
        Some(VwapCategory {
            position: VwapPosition::At,
            extension: Some(VwapExtension::Near),
        })
    );
    assert_opt_f64_bits_eq(vw_at.evidence.distance, Some(0.0), "vw_at distance");

    // Below and Extended: close = 98.0, distance = -1.0, abs >= 1.0 -> Extended
    let vw_bel_ext = classify_vwap(98.0, atr, vwap_out, &config, None);
    assert_eq!(vw_bel_ext.status, ContextReadingStatus::Available);
    assert_eq!(
        vw_bel_ext.category,
        Some(VwapCategory {
            position: VwapPosition::Below,
            extension: Some(VwapExtension::Extended),
        })
    );
    assert_opt_f64_bits_eq(
        vw_bel_ext.evidence.distance,
        Some(-1.0),
        "vw_bel_ext distance",
    );

    // Zero ATR: position still valid, normalized distance unavailable
    let vw_zero_atr = classify_vwap(105.0, 0.0, vwap_out, &config, None);
    assert_eq!(vw_zero_atr.status, ContextReadingStatus::Available);
    assert_eq!(
        vw_zero_atr.category,
        Some(VwapCategory {
            position: VwapPosition::Above,
            extension: None,
        })
    );
    assert_eq!(vw_zero_atr.evidence.distance, None);

    // Caller disabled VWAP
    let vw_disabled = classify_vwap(105.0, atr, None, &config, Some("vwap unavailable"));
    assert_eq!(vw_disabled.status, ContextReadingStatus::Unavailable);
    assert_eq!(vw_disabled.category, None);
    assert_eq!(vw_disabled.reason, Some("vwap unavailable"));
    assert_eq!(vw_disabled.evidence.vwap, None);
    assert_eq!(vw_disabled.evidence.distance, None);

    // Missing volume
    let vw_no_vol = classify_vwap(105.0, atr, None, &config, Some("missing volume"));
    assert_eq!(vw_no_vol.status, ContextReadingStatus::Unavailable);
    assert_eq!(vw_no_vol.category, None);
    assert_eq!(vw_no_vol.reason, Some("missing volume"));

    // Zero session volume (vwap NaN)
    let vw_zero_vol = classify_vwap(
        105.0,
        atr,
        Some(SessionVwapOutput {
            vwap: f64::NAN,
            std_dev: f64::NAN,
        }),
        &config,
        None,
    );
    assert_eq!(vw_zero_vol.status, ContextReadingStatus::Unavailable);
    assert_eq!(vw_zero_vol.category, None);
    assert_eq!(vw_zero_vol.reason, Some("zero session volume"));
}

// -------------------------------------------------------------------------
// 2. Parity, Preview Equivalence, and Repeated Preview Invariance
// -------------------------------------------------------------------------

fn make_synthetic_bars(count: usize) -> Vec<ContextInput> {
    let mut bars = Vec::with_capacity(count);
    let mut price = 100.0;
    for i in 0..count {
        let delta = if i % 2 == 0 { 0.5 } else { -0.3 };
        price += delta;
        let high = price + 1.0;
        let low = price - 1.0;
        let open = price - 0.2;
        let close = price + 0.2;
        let session = (i / 50) as i64;
        let vol_step = (i as f64) * 10.0;
        let vol = 1000.0 + vol_step;
        bars.push(ContextInput::new(
            open,
            high,
            low,
            close,
            Some(vol),
            session,
            true,
        ));
    }
    bars
}

#[test]
fn test_batch_and_streaming_bitwise_parity_and_preview_equivalence() {
    let config = ContextConfig::default();
    let bars = make_synthetic_bars(80);

    // Batch evaluation
    let batch_outputs = batch_context(&bars, &config).expect("batch_context failed");
    assert_eq!(batch_outputs.len(), bars.len());

    let mut state = ContextState::new(config.clone()).expect("ContextState::new failed");

    for (i, bar) in bars.iter().enumerate() {
        // Repeated forming preview tests:
        // Preview with a completely different forming bar value
        let bogus_bar = ContextInput::new(
            bar.open + 10.0,
            bar.high + 20.0,
            bar.low - 5.0,
            bar.close + 15.0,
            bar.volume.map(|v| v * 3.0),
            bar.session_key,
            bar.vwap_available,
        );
        let _ = state.preview(bogus_bar).expect("preview bogus failed");

        // Another preview with lower prices
        let bogus_bar2 = ContextInput::new(
            bar.open - 10.0,
            bar.high + 1.0,
            bar.low - 15.0,
            bar.close - 12.0,
            bar.volume,
            bar.session_key,
            bar.vwap_available,
        );
        let _ = state.preview(bogus_bar2).expect("preview bogus2 failed");

        // Preview with the actual incoming bar
        let preview_out = state.preview(*bar).expect("preview bar failed");

        // Commit the actual incoming bar
        let commit_out = state.commit(*bar).expect("commit bar failed");

        // 1. Preview equals committing the same input
        assert_context_outputs_bits_eq(
            &preview_out,
            &commit_out,
            &format!("bar {i} preview vs commit parity"),
        );

        // 2. Commit equals batch output bitwise
        assert_context_outputs_bits_eq(
            &commit_out,
            &batch_outputs[i],
            &format!("bar {i} commit vs batch parity"),
        );
    }

    // Reset and replay bitwise parity
    state.reset();
    for (i, bar) in bars.iter().enumerate() {
        let replayed = state.commit(*bar).expect("commit on replay failed");
        assert_context_outputs_bits_eq(
            &replayed,
            &batch_outputs[i],
            &format!("bar {i} replay parity"),
        );
    }
}

// -------------------------------------------------------------------------
// 3. Warm-up, Zero Baselines, Unavailable VWAP, and Session Change
// -------------------------------------------------------------------------

#[test]
fn test_warmup_and_partial_availability() {
    let config = ContextConfig {
        fast_period: 2,
        slow_period: 4,
        rsi_period: 3,
        rsi_lower: 30.0,
        rsi_upper: 70.0,
        atr_period: 3,
        volatility_baseline_period: 3,
        volatility_lower: 0.8,
        volatility_upper: 1.2,
        vwap_extension: 1.0,
    };

    let mut state = ContextState::new(config).expect("init failed");

    // Bar 0: initial bar
    let b0 = ContextInput::new(100.0, 101.0, 99.0, 100.0, Some(500.0), 1, true);
    let out0 = state.commit(b0).expect("b0 failed");

    // Trend: warming up on bar 0 (slope needs prior committed bar)
    assert_eq!(out0.trend.status, ContextReadingStatus::WarmingUp);
    assert_eq!(out0.trend.reason, Some("prior bar required for slope"));
    assert_eq!(out0.trend.category, None);

    // Momentum: warming up on bar 0 (rsi needs 3 periods)
    assert_eq!(out0.momentum.status, ContextReadingStatus::WarmingUp);
    assert_eq!(out0.momentum.reason, Some("rsi warming up"));
    assert_eq!(out0.momentum.category, None);

    // Volatility: warming up on bar 0 (atr baseline needs warmup)
    assert_eq!(out0.volatility.status, ContextReadingStatus::WarmingUp);
    assert_eq!(
        out0.volatility.reason,
        Some("volatility baseline warming up")
    );
    assert_eq!(out0.volatility.category, None);

    // VWAP: available on bar 0 because volume is present
    assert_eq!(out0.vwap.status, ContextReadingStatus::Available);
    assert_eq!(out0.vwap.reason, None);
    assert_opt_f64_bits_eq(out0.vwap.evidence.vwap, Some(100.0), "out0 vwap");
    // Since ATR is not yet available, distance is None
    assert_eq!(out0.vwap.evidence.distance, None);

    // Bar 1: slopes are now available!
    let b1 = ContextInput::new(100.0, 102.0, 99.5, 101.0, Some(500.0), 1, true);
    let out1 = state.commit(b1).expect("b1 failed");
    assert_eq!(out1.trend.status, ContextReadingStatus::Available);
    assert_eq!(out1.trend.category, Some(TrendCategory::Upward));

    // Session change on bar 2: VWAP resets, session 2
    let b2 = ContextInput::new(101.0, 103.0, 100.5, 102.0, Some(200.0), 2, true);
    let out2 = state.commit(b2).expect("b2 failed");
    // TP = (103 + 100.5 + 102) / 3 = 101.83333333333333
    let tp = (103.0 + 100.5 + 102.0) / 3.0;
    let expected_vwap = (200.0 * tp) / 200.0;
    assert_f64_bits_eq(
        out2.vwap.evidence.vwap.unwrap(),
        expected_vwap,
        "session change vwap reset",
    );

    // Bar with VWAP unavailable from caller: other readings are completely unaffected
    let b3 = ContextInput::new(102.0, 104.0, 101.5, 103.0, Some(200.0), 2, false);
    let out3 = state.commit(b3).expect("b3 failed");
    assert_eq!(out3.vwap.status, ContextReadingStatus::Unavailable);
    assert_eq!(out3.vwap.reason, Some("vwap unavailable"));
    assert_eq!(out3.trend.status, ContextReadingStatus::Available);
    assert_eq!(out3.trend.category, Some(TrendCategory::Upward));

    // Bar with missing volume: only VWAP is unavailable
    let b4 = ContextInput::new(103.0, 105.0, 102.5, 104.0, None, 2, true);
    let out4 = state.commit(b4).expect("b4 failed");
    assert_eq!(out4.vwap.status, ContextReadingStatus::Unavailable);
    assert_eq!(out4.vwap.reason, Some("missing volume"));
    assert_eq!(out4.trend.status, ContextReadingStatus::Available);
}

// -------------------------------------------------------------------------
// 4. Invalid Config and Input State Preservation
// -------------------------------------------------------------------------

#[test]
fn test_invalid_config_rejection() {
    let base = ContextConfig::default();

    // Fast >= Slow
    let mut cfg = base.clone();
    cfg.fast_period = 21;
    cfg.slow_period = 9;
    assert!(cfg.validate().is_err());
    assert!(ContextState::new(cfg).is_err());

    // Period out of 1..=1000 range
    let mut cfg2 = base.clone();
    cfg2.fast_period = 0;
    assert!(cfg2.validate().is_err());

    let mut cfg3 = base.clone();
    cfg3.slow_period = 1001;
    assert!(cfg3.validate().is_err());

    // RSI bounds
    let mut cfg4 = base.clone();
    cfg4.rsi_lower = 70.0;
    cfg4.rsi_upper = 30.0;
    assert!(cfg4.validate().is_err());

    let mut cfg5 = base.clone();
    cfg5.rsi_lower = -5.0;
    assert!(cfg5.validate().is_err());

    let mut cfg6 = base.clone();
    cfg6.rsi_upper = 105.0;
    assert!(cfg6.validate().is_err());

    // Volatility bounds
    let mut cfg7 = base.clone();
    cfg7.volatility_lower = 1.5;
    cfg7.volatility_upper = 1.0;
    assert!(cfg7.validate().is_err());

    // VWAP extension <= 0
    let mut cfg8 = base.clone();
    cfg8.vwap_extension = 0.0;
    assert!(cfg8.validate().is_err());

    let mut cfg9 = base;
    cfg9.vwap_extension = -1.0;
    assert!(cfg9.validate().is_err());
}

#[test]
fn test_invalid_input_preserves_state() {
    let config = ContextConfig::default();
    let mut state = ContextState::new(config).expect("init failed");

    // Commit 1 valid bar
    let b0 = ContextInput::new(100.0, 101.0, 99.0, 100.5, Some(500.0), 1, true);
    let _out0 = state.commit(b0).expect("b0 failed");

    // Attempt invalid inputs:
    // 1. low > high
    let bad1 = ContextInput::new(100.0, 98.0, 102.0, 100.0, Some(100.0), 1, true);
    assert!(state.commit(bad1).is_err());
    assert!(state.preview(bad1).is_err());

    // 2. open > high
    let bad2 = ContextInput::new(105.0, 102.0, 98.0, 100.0, Some(100.0), 1, true);
    assert!(state.commit(bad2).is_err());

    // 3. close < low
    let bad3 = ContextInput::new(100.0, 102.0, 98.0, 97.0, Some(100.0), 1, true);
    assert!(state.commit(bad3).is_err());

    // 4. negative volume
    let bad4 = ContextInput::new(100.0, 102.0, 98.0, 100.0, Some(-10.0), 1, true);
    assert!(state.commit(bad4).is_err());

    // 5. non-finite price
    let bad5 = ContextInput::new(f64::NAN, 102.0, 98.0, 100.0, Some(100.0), 1, true);
    assert!(state.commit(bad5).is_err());

    // 6. non-positive price
    let bad6 = ContextInput::new(0.0, 102.0, 98.0, 100.0, Some(100.0), 1, true);
    assert!(state.commit(bad6).is_err());

    // Now commit a valid bar 1: state must NOT have been corrupted by bad inputs!
    let b1 = ContextInput::new(100.5, 102.0, 100.0, 101.5, Some(500.0), 1, true);
    let out1 = state.commit(b1).expect("b1 failed");

    assert_eq!(out1.trend.status, ContextReadingStatus::Available);
    // Slope should be compared to out0's fast/slow EMA
    assert!(out1.trend.evidence.fast_slope.is_some());
    assert!(out1.trend.evidence.slow_slope.is_some());
}

// -------------------------------------------------------------------------
// 5. Double-Run Determinism and Prefix Causality
// -------------------------------------------------------------------------

#[test]
fn test_double_run_determinism() {
    let config = ContextConfig::default();
    let bars = make_synthetic_bars(100);

    let run1 = batch_context(&bars, &config).expect("run 1 failed");
    let run2 = batch_context(&bars, &config).expect("run 2 failed");

    assert_eq!(run1.len(), run2.len());
    for (i, (o1, o2)) in run1.iter().zip(run2.iter()).enumerate() {
        assert_context_outputs_bits_eq(o1, o2, &format!("bar {i} double-run determinism"));
    }

    // Verify through q_parity determinism check on fast EMA series
    let fast_emas: Vec<f64> = run1.iter().map(|o| o.trend.evidence.fast_ema).collect();
    check_double_run_with(|| fast_emas.clone()).expect("fast EMA double run failed");
}

#[test]
fn test_prefix_causality() {
    let config = ContextConfig::default();
    let bars = make_synthetic_bars(60);

    let full_run = batch_context(&bars, &config).expect("full batch failed");

    // Any future suffix cannot alter earlier context outputs
    for len in [1, 5, 10, 25, 40, 60] {
        let prefix_run = batch_context(&bars[..len], &config).expect("prefix batch failed");
        for i in 0..len {
            assert_context_outputs_bits_eq(
                &prefix_run[i],
                &full_run[i],
                &format!("prefix len {len} at bar {i} causality"),
            );
        }
    }
}
