//! Python projection of `q-indicators` kernels onto contiguous float64 numpy arrays.

use numpy::{PyArray1, PyReadonlyArray1};
use pyo3::prelude::*;
use pyo3::types::PyModule;
use pyo3::Bound;
use q_indicators::IndicatorError;

type F64Array<'py> = Bound<'py, PyArray1<f64>>;
type F64Pair<'py> = (F64Array<'py>, F64Array<'py>);
type F64Triple<'py> = (F64Array<'py>, F64Array<'py>, F64Array<'py>);

fn contiguous<'a>(
    argument: &'static str,
    array: &'a PyReadonlyArray1<'_, f64>,
) -> PyResult<&'a [f64]> {
    array.as_slice().map_err(|_| {
        PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
            "{argument}: array must be C-contiguous float64"
        ))
    })
}

fn contiguous_i64<'a>(
    argument: &'static str,
    array: &'a PyReadonlyArray1<'_, i64>,
) -> PyResult<&'a [i64]> {
    array.as_slice().map_err(|_| {
        PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
            "{argument}: array must be C-contiguous int64"
        ))
    })
}

fn indicator_error(err: IndicatorError) -> PyErr {
    PyErr::new::<pyo3::exceptions::PyValueError, _>(err.to_string())
}

fn to_array<'py>(py: Python<'py>, values: Vec<f64>) -> Bound<'py, PyArray1<f64>> {
    PyArray1::from_vec(py, values)
}

#[pyfunction]
#[pyo3(signature = (close, window, periods_per_year=252))]
fn realized_vol<'py>(
    py: Python<'py>,
    close: PyReadonlyArray1<'py, f64>,
    window: i64,
    periods_per_year: i64,
) -> PyResult<Bound<'py, PyArray1<f64>>> {
    let close = contiguous("close", &close)?;
    let out =
        q_indicators::realized_vol(close, window, periods_per_year).map_err(indicator_error)?;
    Ok(to_array(py, out))
}

#[pyfunction]
#[pyo3(signature = (open, high, low, close, window, periods_per_year=252))]
fn yang_zhang<'py>(
    py: Python<'py>,
    open: PyReadonlyArray1<'py, f64>,
    high: PyReadonlyArray1<'py, f64>,
    low: PyReadonlyArray1<'py, f64>,
    close: PyReadonlyArray1<'py, f64>,
    window: i64,
    periods_per_year: i64,
) -> PyResult<Bound<'py, PyArray1<f64>>> {
    let open = contiguous("open", &open)?;
    let high = contiguous("high", &high)?;
    let low = contiguous("low", &low)?;
    let close = contiguous("close", &close)?;
    let out = q_indicators::yang_zhang(open, high, low, close, window, periods_per_year)
        .map_err(indicator_error)?;
    Ok(to_array(py, out))
}

#[pyfunction]
fn rsi<'py>(
    py: Python<'py>,
    close: PyReadonlyArray1<'py, f64>,
    period: i64,
) -> PyResult<Bound<'py, PyArray1<f64>>> {
    let close = contiguous("close", &close)?;
    let out = q_indicators::rsi(close, period).map_err(indicator_error)?;
    Ok(to_array(py, out))
}

#[pyfunction]
fn bollinger_bands<'py>(
    py: Python<'py>,
    close: PyReadonlyArray1<'py, f64>,
    period: i64,
    num_std: f64,
) -> PyResult<F64Triple<'py>> {
    let close = contiguous("close", &close)?;
    let bands = q_indicators::bollinger_bands(close, period, num_std).map_err(indicator_error)?;
    Ok((
        to_array(py, bands.upper),
        to_array(py, bands.middle),
        to_array(py, bands.lower),
    ))
}

#[pyfunction]
fn macd<'py>(
    py: Python<'py>,
    close: PyReadonlyArray1<'py, f64>,
    fast_period: i64,
    slow_period: i64,
    signal_period: i64,
) -> PyResult<F64Triple<'py>> {
    let close = contiguous("close", &close)?;
    let macd = q_indicators::macd(close, fast_period, slow_period, signal_period)
        .map_err(indicator_error)?;
    Ok((
        to_array(py, macd.line),
        to_array(py, macd.signal),
        to_array(py, macd.histogram),
    ))
}

#[pyfunction]
fn donchian_channels<'py>(
    py: Python<'py>,
    high: PyReadonlyArray1<'py, f64>,
    low: PyReadonlyArray1<'py, f64>,
    period: i64,
) -> PyResult<F64Pair<'py>> {
    let high = contiguous("high", &high)?;
    let low = contiguous("low", &low)?;
    let ch = q_indicators::donchian_channels(high, low, period).map_err(indicator_error)?;
    Ok((to_array(py, ch.upper), to_array(py, ch.lower)))
}

#[pyfunction]
fn atr<'py>(
    py: Python<'py>,
    high: PyReadonlyArray1<'py, f64>,
    low: PyReadonlyArray1<'py, f64>,
    close: PyReadonlyArray1<'py, f64>,
    period: i64,
) -> PyResult<Bound<'py, PyArray1<f64>>> {
    let high = contiguous("high", &high)?;
    let low = contiguous("low", &low)?;
    let close = contiguous("close", &close)?;
    let out = q_indicators::atr(high, low, close, period).map_err(indicator_error)?;
    Ok(to_array(py, out))
}

#[pyfunction]
fn sma<'py>(
    py: Python<'py>,
    values: PyReadonlyArray1<'py, f64>,
    period: i64,
) -> PyResult<Bound<'py, PyArray1<f64>>> {
    let values = contiguous("values", &values)?;
    let out = q_indicators::sma(values, period).map_err(indicator_error)?;
    Ok(to_array(py, out))
}

#[pyfunction]
fn ema<'py>(
    py: Python<'py>,
    values: PyReadonlyArray1<'py, f64>,
    period: i64,
) -> PyResult<Bound<'py, PyArray1<f64>>> {
    let values = contiguous("values", &values)?;
    let out = q_indicators::ema(values, period).map_err(indicator_error)?;
    Ok(to_array(py, out))
}

#[pyfunction]
fn smma<'py>(
    py: Python<'py>,
    values: PyReadonlyArray1<'py, f64>,
    period: i64,
) -> PyResult<Bound<'py, PyArray1<f64>>> {
    let values = contiguous("values", &values)?;
    let out = q_indicators::smma(values, period).map_err(indicator_error)?;
    Ok(to_array(py, out))
}

#[pyfunction]
fn wma<'py>(
    py: Python<'py>,
    values: PyReadonlyArray1<'py, f64>,
    period: i64,
) -> PyResult<Bound<'py, PyArray1<f64>>> {
    let values = contiguous("values", &values)?;
    let out = q_indicators::wma(values, period).map_err(indicator_error)?;
    Ok(to_array(py, out))
}

#[pyfunction]
fn hma<'py>(
    py: Python<'py>,
    values: PyReadonlyArray1<'py, f64>,
    period: i64,
) -> PyResult<Bound<'py, PyArray1<f64>>> {
    let values = contiguous("values", &values)?;
    let out = q_indicators::hma(values, period).map_err(indicator_error)?;
    Ok(to_array(py, out))
}

#[pyfunction]
fn rolling_zscore<'py>(
    py: Python<'py>,
    values: PyReadonlyArray1<'py, f64>,
    window: i64,
) -> PyResult<Bound<'py, PyArray1<f64>>> {
    let values = contiguous("values", &values)?;
    let out = q_indicators::rolling_zscore(values, window).map_err(indicator_error)?;
    Ok(to_array(py, out))
}

#[pyfunction]
fn rolling_rank<'py>(
    py: Python<'py>,
    values: PyReadonlyArray1<'py, f64>,
    window: i64,
) -> PyResult<Bound<'py, PyArray1<f64>>> {
    let values = contiguous("values", &values)?;
    let out = q_indicators::rolling_rank(values, window).map_err(indicator_error)?;
    Ok(to_array(py, out))
}

#[pyfunction]
fn pct_change<'py>(
    py: Python<'py>,
    values: PyReadonlyArray1<'py, f64>,
    change_bars: i64,
) -> PyResult<Bound<'py, PyArray1<f64>>> {
    let values = contiguous("values", &values)?;
    let out = q_indicators::pct_change(values, change_bars).map_err(indicator_error)?;
    Ok(to_array(py, out))
}

#[pyfunction]
fn clip<'py>(
    py: Python<'py>,
    values: PyReadonlyArray1<'py, f64>,
    low: f64,
    high: f64,
) -> PyResult<Bound<'py, PyArray1<f64>>> {
    let values = contiguous("values", &values)?;
    let out = q_indicators::clip(values, low, high).map_err(indicator_error)?;
    Ok(to_array(py, out))
}

#[pyfunction]
fn log_return<'py>(
    py: Python<'py>,
    close: PyReadonlyArray1<'py, f64>,
    horizon: i64,
) -> PyResult<Bound<'py, PyArray1<f64>>> {
    let close = contiguous("close", &close)?;
    let out = q_indicators::log_return(close, horizon).map_err(indicator_error)?;
    Ok(to_array(py, out))
}

#[pyfunction]
fn realized_vol_per_bar<'py>(
    py: Python<'py>,
    close: PyReadonlyArray1<'py, f64>,
    window: i64,
) -> PyResult<Bound<'py, PyArray1<f64>>> {
    let close = contiguous("close", &close)?;
    let out = q_indicators::realized_vol_per_bar(close, window).map_err(indicator_error)?;
    Ok(to_array(py, out))
}

#[pyfunction]
fn ewma_vol_per_bar<'py>(
    py: Python<'py>,
    close: PyReadonlyArray1<'py, f64>,
    half_life: f64,
) -> PyResult<Bound<'py, PyArray1<f64>>> {
    let close = contiguous("close", &close)?;
    let out = q_indicators::ewma_vol_per_bar(close, half_life).map_err(indicator_error)?;
    Ok(to_array(py, out))
}

#[pyfunction]
fn rogers_satchell<'py>(
    py: Python<'py>,
    open: PyReadonlyArray1<'py, f64>,
    high: PyReadonlyArray1<'py, f64>,
    low: PyReadonlyArray1<'py, f64>,
    close: PyReadonlyArray1<'py, f64>,
    window: i64,
) -> PyResult<Bound<'py, PyArray1<f64>>> {
    let open = contiguous("open", &open)?;
    let high = contiguous("high", &high)?;
    let low = contiguous("low", &low)?;
    let close = contiguous("close", &close)?;
    let out =
        q_indicators::rogers_satchell(open, high, low, close, window).map_err(indicator_error)?;
    Ok(to_array(py, out))
}

#[pyfunction]
fn true_range<'py>(
    py: Python<'py>,
    high: PyReadonlyArray1<'py, f64>,
    low: PyReadonlyArray1<'py, f64>,
    close: PyReadonlyArray1<'py, f64>,
) -> PyResult<Bound<'py, PyArray1<f64>>> {
    let high = contiguous("high", &high)?;
    let low = contiguous("low", &low)?;
    let close = contiguous("close", &close)?;
    let out = q_indicators::true_range(high, low, close).map_err(indicator_error)?;
    Ok(to_array(py, out))
}

#[pyfunction]
fn atr_fraction<'py>(
    py: Python<'py>,
    high: PyReadonlyArray1<'py, f64>,
    low: PyReadonlyArray1<'py, f64>,
    close: PyReadonlyArray1<'py, f64>,
    period: i64,
) -> PyResult<Bound<'py, PyArray1<f64>>> {
    let high = contiguous("high", &high)?;
    let low = contiguous("low", &low)?;
    let close = contiguous("close", &close)?;
    let out = q_indicators::atr_fraction(high, low, close, period).map_err(indicator_error)?;
    Ok(to_array(py, out))
}

#[pyfunction]
fn price_zscore_log<'py>(
    py: Python<'py>,
    close: PyReadonlyArray1<'py, f64>,
    window: i64,
) -> PyResult<Bound<'py, PyArray1<f64>>> {
    let close = contiguous("close", &close)?;
    let out = q_indicators::price_zscore_log(close, window).map_err(indicator_error)?;
    Ok(to_array(py, out))
}

#[pyfunction]
fn ewma_trend_spread<'py>(
    py: Python<'py>,
    close: PyReadonlyArray1<'py, f64>,
    fast_half_life: f64,
    slow_half_life: f64,
) -> PyResult<Bound<'py, PyArray1<f64>>> {
    let close = contiguous("close", &close)?;
    let out = q_indicators::ewma_trend_spread(close, fast_half_life, slow_half_life)
        .map_err(indicator_error)?;
    Ok(to_array(py, out))
}

#[pyfunction]
fn vol_normalize<'py>(
    py: Python<'py>,
    values: PyReadonlyArray1<'py, f64>,
    sigma: PyReadonlyArray1<'py, f64>,
    horizon: i64,
    vol_lag: i64,
) -> PyResult<Bound<'py, PyArray1<f64>>> {
    let values = contiguous("values", &values)?;
    let sigma = contiguous("sigma", &sigma)?;
    let out =
        q_indicators::vol_normalize(values, sigma, horizon, vol_lag).map_err(indicator_error)?;
    Ok(to_array(py, out))
}

#[pyfunction]
fn volatility_log_ratio<'py>(
    py: Python<'py>,
    sigma_short: PyReadonlyArray1<'py, f64>,
    sigma_long: PyReadonlyArray1<'py, f64>,
) -> PyResult<Bound<'py, PyArray1<f64>>> {
    let sigma_short = contiguous("sigma_short", &sigma_short)?;
    let sigma_long = contiguous("sigma_long", &sigma_long)?;
    let out =
        q_indicators::volatility_log_ratio(sigma_short, sigma_long).map_err(indicator_error)?;
    Ok(to_array(py, out))
}

type F64Quad<'py> = (F64Array<'py>, F64Array<'py>, F64Array<'py>, F64Array<'py>);

#[pyfunction]
#[expect(
    clippy::too_many_arguments,
    reason = "quote_spread_bars mirrors the Rust kernel column list"
)]
fn quote_spread_bars<'py>(
    py: Python<'py>,
    time_msc: PyReadonlyArray1<'py, i64>,
    bid: PyReadonlyArray1<'py, f64>,
    ask: PyReadonlyArray1<'py, f64>,
    flags: PyReadonlyArray1<'py, i64>,
    session: PyReadonlyArray1<'py, i64>,
    bar_open_msc: PyReadonlyArray1<'py, i64>,
    bar_close_msc: PyReadonlyArray1<'py, i64>,
    max_quote_age_ms: i64,
) -> PyResult<F64Quad<'py>> {
    let time_msc = contiguous_i64("time_msc", &time_msc)?;
    let bid = contiguous("bid", &bid)?;
    let ask = contiguous("ask", &ask)?;
    let flags = contiguous_i64("flags", &flags)?;
    let session = contiguous_i64("session", &session)?;
    let bar_open_msc = contiguous_i64("bar_open_msc", &bar_open_msc)?;
    let bar_close_msc = contiguous_i64("bar_close_msc", &bar_close_msc)?;
    let out = q_indicators::quote_spread_bars(
        time_msc,
        bid,
        ask,
        flags,
        session,
        bar_open_msc,
        bar_close_msc,
        max_quote_age_ms,
    )
    .map_err(indicator_error)?;
    Ok((
        to_array(py, out.last),
        to_array(py, out.time_weighted_mean),
        to_array(py, out.max),
        to_array(py, out.coverage),
    ))
}

pub(crate) fn register(parent: &Bound<'_, PyModule>) -> PyResult<()> {
    let m = PyModule::new(parent.py(), "indicators")?;
    m.add_function(wrap_pyfunction!(realized_vol, &m)?)?;
    m.add_function(wrap_pyfunction!(yang_zhang, &m)?)?;
    m.add_function(wrap_pyfunction!(rsi, &m)?)?;
    m.add_function(wrap_pyfunction!(bollinger_bands, &m)?)?;
    m.add_function(wrap_pyfunction!(macd, &m)?)?;
    m.add_function(wrap_pyfunction!(donchian_channels, &m)?)?;
    m.add_function(wrap_pyfunction!(atr, &m)?)?;
    m.add_function(wrap_pyfunction!(sma, &m)?)?;
    m.add_function(wrap_pyfunction!(ema, &m)?)?;
    m.add_function(wrap_pyfunction!(smma, &m)?)?;
    m.add_function(wrap_pyfunction!(wma, &m)?)?;
    m.add_function(wrap_pyfunction!(hma, &m)?)?;
    m.add_function(wrap_pyfunction!(rolling_zscore, &m)?)?;
    m.add_function(wrap_pyfunction!(rolling_rank, &m)?)?;
    m.add_function(wrap_pyfunction!(pct_change, &m)?)?;
    m.add_function(wrap_pyfunction!(clip, &m)?)?;
    m.add_function(wrap_pyfunction!(log_return, &m)?)?;
    m.add_function(wrap_pyfunction!(realized_vol_per_bar, &m)?)?;
    m.add_function(wrap_pyfunction!(ewma_vol_per_bar, &m)?)?;
    m.add_function(wrap_pyfunction!(rogers_satchell, &m)?)?;
    m.add_function(wrap_pyfunction!(true_range, &m)?)?;
    m.add_function(wrap_pyfunction!(atr_fraction, &m)?)?;
    m.add_function(wrap_pyfunction!(price_zscore_log, &m)?)?;
    m.add_function(wrap_pyfunction!(ewma_trend_spread, &m)?)?;
    m.add_function(wrap_pyfunction!(vol_normalize, &m)?)?;
    m.add_function(wrap_pyfunction!(volatility_log_ratio, &m)?)?;
    m.add_function(wrap_pyfunction!(quote_spread_bars, &m)?)?;
    parent.add_submodule(&m)?;
    parent
        .py()
        .import("sys")?
        .getattr("modules")?
        .set_item("q_core.indicators", &m)?;
    Ok(())
}
