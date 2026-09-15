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
    parent.add_submodule(&m)?;
    parent
        .py()
        .import("sys")?
        .getattr("modules")?
        .set_item("q_core.indicators", &m)?;
    Ok(())
}
