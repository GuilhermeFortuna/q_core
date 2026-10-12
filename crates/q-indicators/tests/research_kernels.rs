#![forbid(unsafe_code)]

use q_indicators::{
    atr_fraction, ewma_trend_spread, ewma_vol_per_bar, log_return, price_zscore_log,
    realized_vol_per_bar, rogers_satchell, true_range, vol_normalize, volatility_log_ratio,
    yang_zhang, IndicatorError,
};
use q_parity::determinism::check_double_run_with;

const TOL: f64 = 1e-12;

fn assert_close(a: f64, e: f64, ctx: &str) {
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

fn assert_bits_slice(actual: &[f64], expected: &[f64], ctx: &str) {
    assert_eq!(actual.len(), expected.len(), "{ctx}: length");
    for (i, (&a, &e)) in actual.iter().zip(expected.iter()).enumerate() {
        if e.is_nan() {
            assert!(a.is_nan(), "{ctx}[{i}]: expected NaN, got {a}");
        } else {
            assert_eq!(
                a.to_bits(),
                e.to_bits(),
                "{ctx}[{i}]: got {a}, expected {e}"
            );
        }
    }
}

fn prefix_causal_vec<F>(mut run: F, n: usize, ctx: &str)
where
    F: FnMut(usize) -> Vec<f64>,
{
    let full = run(n);
    for len in 1..=n {
        let prefix = run(len);
        for i in 0..len {
            let a = prefix[i];
            let f = full[i];
            if f.is_nan() {
                assert!(a.is_nan(), "{ctx} prefix {len} index {i}");
            } else {
                assert_eq!(a.to_bits(), f.to_bits(), "{ctx} prefix {len} index {i}");
            }
        }
    }
}

#[test]
fn criterion_1_log_return() {
    let close = [100.0, 110.0, 121.0];
    let h1 = log_return(&close, 1).unwrap();
    assert!(h1[0].is_nan());
    assert_close(h1[1], (1.1f64).ln(), "h=1 at index 1");
    assert_close(h1[2], (121.0_f64 / 110.0_f64).ln(), "h=1 at index 2");

    let h2 = log_return(&close, 2).unwrap();
    assert_close(h2[2], (1.21f64).ln(), "h=2 last");

    let flat = [5.0, 5.0, 5.0, 5.0];
    let lr = log_return(&flat, 1).unwrap();
    assert_eq!(lr[3].to_bits(), 0.0_f64.to_bits());

    assert!(matches!(
        log_return(&close, 0),
        Err(IndicatorError::InvalidParameter { .. })
    ));
}

#[test]
fn criterion_2_realized_vol_per_bar() {
    let rets = [0.01_f64, -0.01, 0.01, -0.01];
    let mut close = vec![100.0];
    for r in rets {
        close.push(close.last().unwrap() * r.exp());
    }
    let out = realized_vol_per_bar(&close, 4).unwrap();
    assert!(out[0].is_nan() && out[1].is_nan() && out[2].is_nan() && out[3].is_nan());
    assert_close(out[4], 0.01, "rvol n=4");
}

#[test]
fn criterion_3_ewma_vol_per_bar() {
    let mut close = vec![100.0];
    close.push(100.0 * (0.01f64).exp());
    close.push(close[1] * (0.02f64).exp());
    // Internal variance after two returns is verified in `research` unit tests.

    let long: Vec<f64> = (0..300)
        .map(|i| {
            #[expect(
                clippy::suboptimal_flops,
                reason = "synthetic series must not use fused multiply-add"
            )]
            {
                100.0 + i as f64 * 0.01
            }
        })
        .collect();
    let ew = ewma_vol_per_bar(&long, 32.0).unwrap();
    for (i, v) in ew.iter().enumerate().take(214) {
        assert!(v.is_nan(), "index {i} should be NaN before 214");
    }
    let manual = ewma_vol_per_bar(&long[..215], 32.0).unwrap();
    assert_eq!(ew[214].to_bits(), manual[214].to_bits());
}

#[test]
fn criterion_4_rogers_satchell_and_yang_zhang() {
    let prev = 99.0;
    let open = [prev, 100.0, 103.0, 104.0];
    let high = [prev, 102.0, 106.0, 105.0];
    let low = [prev, 98.0, 102.0, 101.0];
    let close = [prev, 101.0, 105.0, 102.0];
    let rs = rogers_satchell(&open[1..], &high[1..], &low[1..], &close[1..], 3).unwrap();
    assert_close(rs[2], 0.02533109341314, "rogers_satchell");
    let yz = yang_zhang(&open, &high, &low, &close, 3, 1).unwrap();
    assert_close(yz[3], 0.02896649184455, "yang_zhang ppy=1");
}

#[test]
fn criterion_5_true_range_and_atr_fraction() {
    let high2 = [100.0, 106.0];
    let low2 = [100.0, 103.0];
    let close2 = [100.0, 100.0];
    let tr2 = true_range(&high2, &low2, &close2).unwrap();
    assert!(tr2[0].is_nan());
    assert_close(tr2[1], 6.0, "true_range");

    let atr_f = atr_fraction(&high2, &low2, &close2, 1).unwrap();
    let atr = q_indicators::atr(&high2, &low2, &close2, 1).unwrap();
    for i in 0..2 {
        if close2[i] > 0.0 && !atr[i].is_nan() {
            assert_close(atr_f[i], atr[i] / close2[i], "atr_fraction");
        }
    }
}

#[test]
fn criterion_6_price_zscore_log() {
    let e = std::f64::consts::E;
    let close = [1.0, e, e * e];
    let z = price_zscore_log(&close, 3).unwrap();
    assert_close(z[2], 1.0, "zscore end");
    let flat = [2.0, 2.0, 2.0];
    let zf = price_zscore_log(&flat, 3).unwrap();
    assert!(zf[2].is_nan());
    let tiny = [1.0, 1.0 + 1e-15, 1.0 + 2e-15];
    let zt = price_zscore_log(&tiny, 3).unwrap();
    assert!(zt[2].is_nan());
}

#[test]
fn criterion_7_ewma_trend_spread() {
    let flat = [10.0, 10.0, 10.0, 10.0, 10.0, 10.0, 10.0, 10.0, 10.0, 10.0];
    let spread = ewma_trend_spread(&flat, 1.0, 4.0).unwrap();
    assert!(spread
        .iter()
        .all(|x| x.is_nan() || x.to_bits() == 0.0_f64.to_bits()));
    assert!(matches!(
        ewma_trend_spread(&flat, 4.0, 4.0),
        Err(IndicatorError::InvalidInput { .. })
    ));
    assert!(matches!(
        ewma_trend_spread(&flat, 5.0, 4.0),
        Err(IndicatorError::InvalidInput { .. })
    ));
}

#[test]
fn criterion_8_vol_normalize() {
    let a = 0.01;
    let values = [0.0, 2.0 * a];
    let sigma = [a, a];
    let out = vol_normalize(&values, &sigma, 2, 0).unwrap();
    assert_close(out[1], (2.0f64).sqrt(), "vol_normalize lag0");
    let tiny = vol_normalize(&[1.0], &[1e-13], 1, 0).unwrap();
    assert!(tiny[0].is_nan());
    let sigma2 = [a, a, a];
    let values2 = [0.0, 0.0, 2.0 * a];
    let lag1 = vol_normalize(&values2, &sigma2, 2, 1).unwrap();
    assert_close(lag1[2], (2.0f64).sqrt(), "vol_normalize lag1");
}

#[test]
fn criterion_9_volatility_log_ratio() {
    let out = volatility_log_ratio(&[0.02], &[0.01]).unwrap();
    assert_close(out[0], (2.0f64).ln(), "vol ratio");
}

#[test]
fn criterion_12_rescale_invariance_and_ewma_restart() {
    let scale = 7.5;
    let open = [100.0, 103.0, 104.0];
    let high = [102.0, 106.0, 105.0];
    let low = [98.0, 102.0, 101.0];
    let close = [101.0, 105.0, 102.0];
    let so = open.iter().map(|x| x * scale).collect::<Vec<_>>();
    let sh = high.iter().map(|x| x * scale).collect::<Vec<_>>();
    let sl = low.iter().map(|x| x * scale).collect::<Vec<_>>();
    let sc = close.iter().map(|x| x * scale).collect::<Vec<_>>();

    let rs1 = rogers_satchell(&open, &high, &low, &close, 2).unwrap();
    let rs2 = rogers_satchell(&so, &sh, &sl, &sc, 2).unwrap();
    assert_bits_slice(&rs2, &rs1, "rogers rescale");

    let tr1 = true_range(&high, &low, &close).unwrap();
    let tr2 = true_range(&sh, &sl, &sc).unwrap();
    assert_close(tr2[1], tr1[1] * scale, "true_range scales");

    let lr1 = log_return(&close, 1).unwrap();
    let lr2 = log_return(&sc, 1).unwrap();
    for i in 0..close.len() {
        assert_close(lr2[i], lr1[i], "log_return rescale");
    }

    let bad = [100.0, -1.0, 110.0, 121.0];
    let ew = ewma_vol_per_bar(&bad, 1.0).unwrap();
    assert!(ew[1].is_nan());
    assert!(ew[2].is_nan() || ew[3].is_nan());

    let _ = check_double_run_with(|| log_return(&close, 1).unwrap());
    let _ = check_double_run_with(|| realized_vol_per_bar(&close, 2).unwrap());
    let _ = check_double_run_with(|| ewma_vol_per_bar(&close, 2.0).unwrap());
    let _ = check_double_run_with(|| rogers_satchell(&open, &high, &low, &close, 2).unwrap());
    let _ = check_double_run_with(|| price_zscore_log(&close, 2).unwrap());
    let _ = check_double_run_with(|| ewma_trend_spread(&close, 1.0, 3.0).unwrap());
    let sigma = [0.01, 0.02, 0.03];
    let _ = check_double_run_with(|| vol_normalize(&close, &sigma, 1, 0).unwrap());
    let _ = check_double_run_with(|| volatility_log_ratio(&sigma, &sigma).unwrap());

    prefix_causal_vec(
        |len| log_return(&close[..len], 1).unwrap(),
        close.len(),
        "log_return",
    );
    prefix_causal_vec(
        |len| realized_vol_per_bar(&close[..len], 2).unwrap(),
        close.len(),
        "realized_vol",
    );
}
