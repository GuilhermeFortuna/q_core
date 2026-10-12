//! Research indicator kernels (log returns, per-bar volatility, normalized scores).

use crate::elementwise::shift;
use crate::error::IndicatorError;
use crate::indicators::atr;
const VOL_EPS: f64 = 1e-12;
/// Largest half-life whose readiness count `N_EW` still fits 32 bits.
const MAX_HALF_LIFE: f64 = 5e8;

fn log_close(price: f64) -> f64 {
    if price.is_finite() && price > 0.0 {
        price.ln()
    } else {
        f64::NAN
    }
}

fn log_closes(close: &[f64]) -> Vec<f64> {
    close.iter().map(|&p| log_close(p)).collect()
}

fn log_returns_from_close(close: &[f64]) -> Vec<f64> {
    let l = log_closes(close);
    let mut r = vec![f64::NAN; close.len()];
    for i in 1..close.len() {
        if l[i].is_nan() || l[i - 1].is_nan() {
            r[i] = f64::NAN;
        } else {
            r[i] = l[i] - l[i - 1];
        }
    }
    r
}

pub(crate) fn ew_n_ready(half_life: f64) -> usize {
    1 + (half_life * 100.0f64.log2()).ceil() as usize
}

fn validate_half_life(function: &'static str, half_life: f64) -> Result<(), IndicatorError> {
    if !half_life.is_finite() || half_life <= 0.0 {
        return Err(IndicatorError::InvalidInput {
            function,
            parameter: "half_life",
            value: half_life,
            requirement: "finite and > 0",
        });
    }
    if half_life > MAX_HALF_LIFE {
        return Err(IndicatorError::InvalidInput {
            function,
            parameter: "half_life",
            value: half_life,
            requirement: "<= 5e8",
        });
    }
    Ok(())
}

fn ewma_half_life(values: &[f64], half_life: f64) -> Vec<f64> {
    let lambda = (-std::f64::consts::LN_2 / half_life).exp();
    let ready = ew_n_ready(half_life);
    let mut out = vec![f64::NAN; values.len()];
    let mut state = f64::NAN;
    let mut seen = 0usize;
    for i in 0..values.len() {
        let x = values[i];
        if x.is_nan() {
            state = f64::NAN;
            seen = 0;
            continue;
        }
        if seen == 0 {
            state = x;
            seen = 1;
            if seen >= ready {
                out[i] = state;
            }
            continue;
        }
        #[expect(
            clippy::suboptimal_flops,
            reason = "research EWMA must not use fused multiply-add"
        )]
        {
            state = lambda * state + (1.0 - lambda) * x;
        }
        seen += 1;
        if seen >= ready {
            out[i] = state;
        }
    }
    out
}

fn ewma_vol_variance(close: &[f64], half_life: f64) -> Vec<f64> {
    let r = log_returns_from_close(close);
    let r2: Vec<f64> = r
        .iter()
        .map(|&x| if x.is_nan() { f64::NAN } else { x * x })
        .collect();
    ewma_half_life(&r2, half_life)
}

fn length_mismatch(
    function: &'static str,
    parameter: &'static str,
    expected: usize,
    actual: usize,
) -> IndicatorError {
    IndicatorError::LengthMismatch {
        function,
        parameter,
        expected,
        actual,
    }
}

fn check_same_len(
    function: &'static str,
    columns: &[(&'static str, usize)],
    expected: usize,
) -> Result<(), IndicatorError> {
    for &(name, len) in columns {
        if len != expected {
            return Err(length_mismatch(function, name, expected, len));
        }
    }
    Ok(())
}

/// Log return `ln(C_t) - ln(C_{t-h})` for `h >= 1`; first value at index `h`.
pub fn log_return(close: &[f64], horizon: i64) -> Result<Vec<f64>, IndicatorError> {
    if horizon < 1 {
        return Err(IndicatorError::InvalidParameter {
            function: "log_return",
            parameter: "horizon",
            value: horizon,
            requirement: ">= 1",
        });
    }
    let h = horizon as usize;
    let l = log_closes(close);
    let mut out = vec![f64::NAN; close.len()];
    for i in h..close.len() {
        if l[i].is_nan() || l[i - h].is_nan() {
            out[i] = f64::NAN;
        } else {
            out[i] = l[i] - l[i - h];
        }
    }
    Ok(out)
}

/// Per-bar realized volatility `sqrt(mean(r^2))` over trailing `window` log returns.
pub fn realized_vol_per_bar(close: &[f64], window: i64) -> Result<Vec<f64>, IndicatorError> {
    if window < 1 {
        return Err(IndicatorError::InvalidParameter {
            function: "realized_vol_per_bar",
            parameter: "window",
            value: window,
            requirement: ">= 1",
        });
    }
    let w = window as usize;
    let r = log_returns_from_close(close);
    let n = close.len();
    let mut out = vec![f64::NAN; n];
    for i in w..n {
        let start = i + 1 - w;
        let slice = &r[start..=i];
        if slice.iter().any(|x| x.is_nan()) {
            continue;
        }
        let mut sum_sq = 0.0;
        for &x in slice {
            #[expect(
                clippy::suboptimal_flops,
                reason = "sum of squares must not use fused multiply-add"
            )]
            {
                sum_sq += x * x;
            }
        }
        out[i] = (sum_sq / (w as f64)).sqrt();
    }
    Ok(out)
}

/// EWMA per-bar volatility from squared log returns; output `sqrt(v)`.
pub fn ewma_vol_per_bar(close: &[f64], half_life: f64) -> Result<Vec<f64>, IndicatorError> {
    validate_half_life("ewma_vol_per_bar", half_life)?;
    let var = ewma_vol_variance(close, half_life);
    Ok(var
        .iter()
        .map(|&v| if v.is_nan() { f64::NAN } else { v.sqrt() })
        .collect())
}

/// Rogers–Satchell per-bar volatility estimator.
pub fn rogers_satchell(
    open: &[f64],
    high: &[f64],
    low: &[f64],
    close: &[f64],
    window: i64,
) -> Result<Vec<f64>, IndicatorError> {
    if window < 1 {
        return Err(IndicatorError::InvalidParameter {
            function: "rogers_satchell",
            parameter: "window",
            value: window,
            requirement: ">= 1",
        });
    }
    let n = close.len();
    check_same_len(
        "rogers_satchell",
        &[
            ("open", open.len()),
            ("high", high.len()),
            ("low", low.len()),
            ("close", n),
        ],
        n,
    )?;
    let w = window as usize;
    let mut terms = vec![f64::NAN; n];
    for i in 0..n {
        if !open[i].is_finite()
            || open[i] <= 0.0
            || !high[i].is_finite()
            || high[i] <= 0.0
            || !low[i].is_finite()
            || low[i] <= 0.0
            || !close[i].is_finite()
            || close[i] <= 0.0
        {
            continue;
        }
        let u = (high[i] / open[i]).ln();
        let d = (low[i] / open[i]).ln();
        let c = (close[i] / open[i]).ln();
        #[expect(
            clippy::suboptimal_flops,
            reason = "pandas-style unfused u*(u-c)+d*(d-c)"
        )]
        {
            terms[i] = u * (u - c) + d * (d - c);
        }
    }
    let mut out = vec![f64::NAN; n];
    for i in (w - 1)..n {
        let start = i + 1 - w;
        let slice = &terms[start..=i];
        if slice.iter().any(|x| x.is_nan()) {
            continue;
        }
        let mut sum = 0.0;
        for &x in slice {
            sum += x;
        }
        out[i] = (sum / (w as f64)).sqrt();
    }
    Ok(out)
}

/// True range per bar; first value at index `1`.
pub fn true_range(high: &[f64], low: &[f64], close: &[f64]) -> Result<Vec<f64>, IndicatorError> {
    let n = close.len();
    check_same_len(
        "true_range",
        &[("high", high.len()), ("low", low.len()), ("close", n)],
        n,
    )?;
    let prev = shift(close, 1);
    let mut out = vec![f64::NAN; n];
    for i in 1..n {
        let tr1 = high[i] - low[i];
        let tr2 = (high[i] - prev[i]).abs();
        let tr3 = (low[i] - prev[i]).abs();
        out[i] = crate::indicators::nan_skipping_max3(tr1, tr2, tr3);
    }
    Ok(out)
}

/// `atr / close` where `close > 0`.
pub fn atr_fraction(
    high: &[f64],
    low: &[f64],
    close: &[f64],
    period: i64,
) -> Result<Vec<f64>, IndicatorError> {
    let atr_vals = atr(high, low, close, period)?;
    let mut out = vec![f64::NAN; close.len()];
    for i in 0..close.len() {
        if close[i] > 0.0 && close[i].is_finite() && !atr_vals[i].is_nan() {
            out[i] = atr_vals[i] / close[i];
        }
    }
    Ok(out)
}

/// Log-price z-score with sample standard deviation (`ddof = 1`).
pub fn price_zscore_log(close: &[f64], window: i64) -> Result<Vec<f64>, IndicatorError> {
    if window < 2 {
        return Err(IndicatorError::InvalidParameter {
            function: "price_zscore_log",
            parameter: "window",
            value: window,
            requirement: ">= 2",
        });
    }
    let w = window as usize;
    let l = log_closes(close);
    let mean = rolling_mean_on_strict(&l, w);
    let std = rolling_std_on_strict(&l, w);
    let mut out = vec![f64::NAN; close.len()];
    for i in 0..close.len() {
        if l[i].is_nan() || mean[i].is_nan() || std[i].is_nan() || std[i] <= VOL_EPS {
            continue;
        }
        out[i] = (l[i] - mean[i]) / std[i];
    }
    Ok(out)
}

fn rolling_mean_on_strict(values: &[f64], window: usize) -> Vec<f64> {
    let n = values.len();
    let mut out = vec![f64::NAN; n];
    for i in (window - 1)..n {
        let start = i + 1 - window;
        let slice = &values[start..=i];
        if slice.iter().any(|x| x.is_nan()) {
            continue;
        }
        let mut sum = 0.0;
        for &x in slice {
            sum += x;
        }
        out[i] = sum / (window as f64);
    }
    out
}

fn rolling_std_on_strict(values: &[f64], window: usize) -> Vec<f64> {
    let n = values.len();
    let mut out = vec![f64::NAN; n];
    if window < 2 {
        return out;
    }
    for i in (window - 1)..n {
        let start = i + 1 - window;
        let slice = &values[start..=i];
        if slice.iter().any(|x| x.is_nan()) {
            continue;
        }
        let mut sum = 0.0;
        for &x in slice {
            sum += x;
        }
        let mean = sum / (window as f64);
        let mut ss = 0.0;
        for &x in slice {
            let d = x - mean;
            ss += d * d;
        }
        let var = ss / ((window - 1) as f64);
        out[i] = if var < 0.0 { 0.0 } else { var.sqrt() };
    }
    out
}

/// EWMA trend spread `EW(log close, fast) - EW(log close, slow)`; requires `fast < slow`.
pub fn ewma_trend_spread(
    close: &[f64],
    fast_half_life: f64,
    slow_half_life: f64,
) -> Result<Vec<f64>, IndicatorError> {
    validate_half_life("ewma_trend_spread", fast_half_life)?;
    validate_half_life("ewma_trend_spread", slow_half_life)?;
    if fast_half_life >= slow_half_life {
        return Err(IndicatorError::InvalidInput {
            function: "ewma_trend_spread",
            parameter: "fast_half_life",
            value: fast_half_life,
            requirement: "< slow_half_life",
        });
    }
    let l = log_closes(close);
    let fast = ewma_half_life(&l, fast_half_life);
    let slow = ewma_half_life(&l, slow_half_life);
    let mut out = vec![f64::NAN; close.len()];
    for i in 0..close.len() {
        if fast[i].is_nan() || slow[i].is_nan() {
            continue;
        }
        out[i] = fast[i] - slow[i];
    }
    Ok(out)
}

/// Normalize values by lagged per-bar volatility and `sqrt(horizon)`.
pub fn vol_normalize(
    values: &[f64],
    sigma: &[f64],
    horizon: i64,
    vol_lag: i64,
) -> Result<Vec<f64>, IndicatorError> {
    if horizon < 1 {
        return Err(IndicatorError::InvalidParameter {
            function: "vol_normalize",
            parameter: "horizon",
            value: horizon,
            requirement: ">= 1",
        });
    }
    if vol_lag != 0 && vol_lag != 1 {
        return Err(IndicatorError::InvalidParameter {
            function: "vol_normalize",
            parameter: "vol_lag",
            value: vol_lag,
            requirement: "0 or 1",
        });
    }
    let n = values.len();
    check_same_len("vol_normalize", &[("sigma", sigma.len())], n)?;
    let lag = vol_lag as usize;
    let scale = (horizon as f64).sqrt();
    let mut out = vec![f64::NAN; n];
    for i in 0..n {
        if i < lag {
            continue;
        }
        let sig = sigma[i - lag];
        if values[i].is_nan() || sig.is_nan() || sig <= VOL_EPS {
            continue;
        }
        out[i] = values[i] / (sig * scale);
    }
    Ok(out)
}

/// `ln(sigma_short) - ln(sigma_long)` when both valid; denominators at or below `1e-12` yield NaN.
pub fn volatility_log_ratio(
    sigma_short: &[f64],
    sigma_long: &[f64],
) -> Result<Vec<f64>, IndicatorError> {
    let n = sigma_short.len();
    check_same_len(
        "volatility_log_ratio",
        &[("sigma_long", sigma_long.len())],
        n,
    )?;
    let mut out = vec![f64::NAN; n];
    for i in 0..n {
        let s = sigma_short[i];
        let l = sigma_long[i];
        if s.is_nan() || l.is_nan() || s <= VOL_EPS || l <= VOL_EPS {
            continue;
        }
        out[i] = s.ln() - l.ln();
    }
    Ok(out)
}
