//! Python projection of `q-engine` tick kernels onto contiguous numpy arrays.

use numpy::{PyArray1, PyReadonlyArray1};
use pyo3::exceptions::{PyTypeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyAnyMethods, PyDict, PyModule, PyModuleMethods};
use pyo3::Bound;
use q_engine::{
    resolve_bar_ms as resolve_bar_ms_rs, sample_at_bar_ends as sample_at_bar_ends_rs,
    simulate_ticks, tick_bars as tick_bars_rs, tick_day_bounds as tick_day_bounds_rs, TickError,
    TickInputs, TickSizing,
};

type I64Array<'py> = Bound<'py, PyArray1<i64>>;
type I64Pair<'py> = (I64Array<'py>, I64Array<'py>);

fn dtype_name(obj: &Bound<'_, PyAny>) -> PyResult<String> {
    Ok(obj.getattr("dtype")?.str()?.to_string_lossy().into_owned())
}

fn contiguous_f64<'a>(
    argument: &'static str,
    array: &'a PyReadonlyArray1<'_, f64>,
) -> PyResult<&'a [f64]> {
    array.as_slice().map_err(|_| {
        PyValueError::new_err(format!("{argument}: array must be C-contiguous float64"))
    })
}

fn contiguous_i64<'a>(
    argument: &'static str,
    array: &'a PyReadonlyArray1<'_, i64>,
) -> PyResult<&'a [i64]> {
    array
        .as_slice()
        .map_err(|_| PyValueError::new_err(format!("{argument}: array must be C-contiguous int64")))
}

fn contiguous_i8<'a>(
    argument: &'static str,
    array: &'a PyReadonlyArray1<'_, i8>,
) -> PyResult<&'a [i8]> {
    array
        .as_slice()
        .map_err(|_| PyValueError::new_err(format!("{argument}: array must be C-contiguous int8")))
}

fn extract_f64<'py>(
    argument: &'static str,
    obj: &Bound<'py, PyAny>,
) -> PyResult<PyReadonlyArray1<'py, f64>> {
    let dtype = dtype_name(obj)?;
    if dtype != "float64" {
        return Err(PyValueError::new_err(format!(
            "{argument}: expected float64, got {dtype}"
        )));
    }
    obj.extract()
}

fn extract_i64<'py>(
    argument: &'static str,
    obj: &Bound<'py, PyAny>,
) -> PyResult<PyReadonlyArray1<'py, i64>> {
    let dtype = dtype_name(obj)?;
    if dtype != "int64" {
        return Err(PyTypeError::new_err(format!(
            "{argument}: expected int64, got {dtype}"
        )));
    }
    obj.extract()
}

fn extract_i8<'py>(
    argument: &'static str,
    obj: &Bound<'py, PyAny>,
) -> PyResult<PyReadonlyArray1<'py, i8>> {
    let dtype = dtype_name(obj)?;
    if dtype != "int8" {
        return Err(PyTypeError::new_err(format!(
            "{argument}: expected int8, got {dtype}"
        )));
    }
    obj.extract()
}

fn tick_error(err: TickError) -> PyErr {
    match err {
        TickError::LengthMismatch {
            column,
            expected,
            actual,
        } => PyValueError::new_err(format!(
            "{column}: length mismatch (expected {expected}, got {actual})"
        )),
        TickError::InvalidSizing { field, reason } => {
            PyValueError::new_err(format!("{field}: {reason}"))
        }
        TickError::VolumeNotFinite { bar } => {
            PyValueError::new_err(format!("volume not finite at bar {bar}"))
        }
    }
}

fn parse_sizing(sizing: &Bound<'_, PyAny>) -> PyResult<TickSizing> {
    let typ: String = sizing.get_item("type")?.extract()?;
    match typ.as_str() {
        "fixed_quantity" => {
            let quantity: f64 = sizing.get_item("quantity")?.extract()?;
            Ok(TickSizing::FixedQuantity { quantity })
        }
        "fixed_safety_margin" => {
            let margin_per_contract: f64 =
                sizing.get_item("safety_margin_per_contract")?.extract()?;
            let min_contracts: i64 = sizing.get_item("min_contracts")?.extract()?;
            let max_obj = sizing.get_item("max_contracts")?;
            let max_contracts = if max_obj.is_none() {
                0
            } else {
                max_obj.extract::<Option<i64>>()?.unwrap_or(0)
            };
            Ok(TickSizing::FixedSafetyMargin {
                margin_per_contract,
                min_contracts,
                max_contracts,
            })
        }
        "inverse_volatility" => Err(PyValueError::new_err(
            "inverse_volatility sizing is not supported for tick simulation",
        )),
        other => Err(PyValueError::new_err(format!(
            "unsupported sizing type: {other}"
        ))),
    }
}

#[pyfunction(signature = (*, bid, ask, direction, sl_points, tp_points, initial_capital, point_value, sizing))]
#[allow(clippy::too_many_arguments)]
fn tick_simulate<'py>(
    py: Python<'py>,
    bid: Bound<'py, PyAny>,
    ask: Bound<'py, PyAny>,
    direction: Bound<'py, PyAny>,
    sl_points: Bound<'py, PyAny>,
    tp_points: Bound<'py, PyAny>,
    initial_capital: f64,
    point_value: f64,
    sizing: Bound<'py, PyAny>,
) -> PyResult<Bound<'py, PyDict>> {
    let bid_a = extract_f64("bid", &bid)?;
    let ask_a = extract_f64("ask", &ask)?;
    let direction_a = extract_i8("direction", &direction)?;
    let sl_a = extract_f64("sl_points", &sl_points)?;
    let tp_a = extract_f64("tp_points", &tp_points)?;

    let bid_s = contiguous_f64("bid", &bid_a)?;
    let ask_s = contiguous_f64("ask", &ask_a)?;
    let direction_s = contiguous_i8("direction", &direction_a)?;
    let sl_s = contiguous_f64("sl_points", &sl_a)?;
    let tp_s = contiguous_f64("tp_points", &tp_a)?;

    let sizing = parse_sizing(&sizing)?;
    let inputs = TickInputs {
        bid: bid_s,
        ask: ask_s,
        direction: direction_s,
        sl_points: sl_s,
        tp_points: tp_s,
    };
    let run = simulate_ticks(&inputs, initial_capital, point_value, sizing).map_err(tick_error)?;

    let out = PyDict::new(py);
    out.set_item("entry_idx", PyArray1::from_vec(py, run.trades.entry_idx))?;
    out.set_item("exit_idx", PyArray1::from_vec(py, run.trades.exit_idx))?;
    out.set_item(
        "entry_price",
        PyArray1::from_vec(py, run.trades.entry_price),
    )?;
    out.set_item("exit_price", PyArray1::from_vec(py, run.trades.exit_price))?;
    out.set_item("direction", PyArray1::from_vec(py, run.trades.direction))?;
    out.set_item("quantity", PyArray1::from_vec(py, run.trades.quantity))?;
    out.set_item(
        "exit_reason",
        PyArray1::from_vec(py, run.trades.exit_reason),
    )?;
    out.set_item("final_capital", run.final_capital)?;
    Ok(out)
}

#[pyfunction]
fn tick_day_bounds<'py>(py: Python<'py>, time_msc: Bound<'py, PyAny>) -> PyResult<I64Pair<'py>> {
    let time_a = extract_i64("time_msc", &time_msc)?;
    let time_s = contiguous_i64("time_msc", &time_a)?;
    let (starts, ends) = tick_day_bounds_rs(time_s);
    Ok((PyArray1::from_vec(py, starts), PyArray1::from_vec(py, ends)))
}

#[pyfunction(signature = (*, time_msc, bid, ask, last, volume, bar_ms))]
fn tick_bars<'py>(
    py: Python<'py>,
    time_msc: Bound<'py, PyAny>,
    bid: Bound<'py, PyAny>,
    ask: Bound<'py, PyAny>,
    last: Bound<'py, PyAny>,
    volume: Bound<'py, PyAny>,
    bar_ms: i64,
) -> PyResult<Bound<'py, PyDict>> {
    let time_a = extract_i64("time_msc", &time_msc)?;
    let bid_a = extract_f64("bid", &bid)?;
    let ask_a = extract_f64("ask", &ask)?;
    let last_a = extract_f64("last", &last)?;
    let volume_a = extract_f64("volume", &volume)?;

    let time_s = contiguous_i64("time_msc", &time_a)?;
    let bid_s = contiguous_f64("bid", &bid_a)?;
    let ask_s = contiguous_f64("ask", &ask_a)?;
    let last_s = contiguous_f64("last", &last_a)?;
    let volume_s = contiguous_f64("volume", &volume_a)?;

    let bars = tick_bars_rs(time_s, bid_s, ask_s, last_s, volume_s, bar_ms).map_err(tick_error)?;

    let out = PyDict::new(py);
    out.set_item("open_msc", PyArray1::from_vec(py, bars.open_msc))?;
    out.set_item("open", PyArray1::from_vec(py, bars.open))?;
    out.set_item("high", PyArray1::from_vec(py, bars.high))?;
    out.set_item("low", PyArray1::from_vec(py, bars.low))?;
    out.set_item("close", PyArray1::from_vec(py, bars.close))?;
    out.set_item("volume", PyArray1::from_vec(py, bars.volume))?;
    out.set_item("tick_start", PyArray1::from_vec(py, bars.tick_start))?;
    out.set_item("tick_end", PyArray1::from_vec(py, bars.tick_end))?;
    Ok(out)
}

#[pyfunction]
fn resolve_bar_ms(base_bar_ms: i64, span_msc: i64) -> i64 {
    resolve_bar_ms_rs(base_bar_ms, span_msc)
}

#[pyfunction]
fn sample_at_bar_ends<'py>(
    py: Python<'py>,
    series: Bound<'py, PyAny>,
    tick_end: Bound<'py, PyAny>,
) -> PyResult<Bound<'py, PyArray1<f64>>> {
    let series_a = extract_f64("series", &series)?;
    let tick_end_a = extract_i64("tick_end", &tick_end)?;
    let series_s = contiguous_f64("series", &series_a)?;
    let tick_end_s = contiguous_i64("tick_end", &tick_end_a)?;
    let out = sample_at_bar_ends_rs(series_s, tick_end_s);
    Ok(PyArray1::from_vec(py, out))
}

pub(crate) fn register(parent: &Bound<'_, PyModule>) -> PyResult<()> {
    let m = PyModule::new(parent.py(), "engine")?;
    m.add_function(wrap_pyfunction!(tick_simulate, &m)?)?;
    m.add_function(wrap_pyfunction!(tick_day_bounds, &m)?)?;
    m.add_function(wrap_pyfunction!(tick_bars, &m)?)?;
    m.add_function(wrap_pyfunction!(resolve_bar_ms, &m)?)?;
    m.add_function(wrap_pyfunction!(sample_at_bar_ends, &m)?)?;
    parent.add_submodule(&m)?;
    parent
        .py()
        .import("sys")?
        .getattr("modules")?
        .set_item("q_core.engine", &m)?;
    Ok(())
}
