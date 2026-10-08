//! Python projection of `q-engine` candle and tick kernels onto contiguous numpy arrays.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};

use numpy::{PyArray1, PyReadonlyArray1};
use pyo3::exceptions::{PyTypeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyAnyMethods, PyBool, PyDict, PyList, PyModule, PyModuleMethods, PyTuple};
use pyo3::Bound;
use pyo3::IntoPyObjectExt;
use q_engine::{
    resolve_bar_ms as resolve_bar_ms_rs, run_candle_with_callbacks,
    sample_at_bar_ends as sample_at_bar_ends_rs, simulate_ticks, tick_bars as tick_bars_rs,
    tick_day_bounds as tick_day_bounds_rs, BarSignals, CandleConfig, CandleError, CandleInputs,
    Costs, DayTradeWindow, DecisionStep, ExitCallbacks, ExitInputs, ExitParams, ExitRuleId,
    ExitRuleSet, IntrabarPrices, IntrabarSource, ParamValue, PositionKey, PositionSnapshot,
    ProtectiveColumns, QueuedEntry, RuleState, RuntimeCallbacks, Side, SignalColumns, Sizing,
    StrategyFn, TickError, TickInputs, TickSizing, TradeView,
};

type I64Array<'py> = Bound<'py, PyArray1<i64>>;
type I64Pair<'py> = (I64Array<'py>, I64Array<'py>);

enum StrategyRunError {
    Kernel(CandleError),
    Python(PyErr),
}
impl From<CandleError> for StrategyRunError {
    fn from(err: CandleError) -> Self {
        Self::Kernel(err)
    }
}
impl From<PyErr> for StrategyRunError {
    fn from(err: PyErr) -> Self {
        Self::Python(err)
    }
}

fn dtype_name(obj: &Bound<'_, PyAny>) -> PyResult<String> {
    Ok(obj.getattr("dtype")?.str()?.to_string_lossy().into_owned())
}

fn candle_error(err: CandleError) -> PyErr {
    match err {
        CandleError::InvalidConfig { field, reason } => {
            PyValueError::new_err(format!("{field}: {reason}"))
        }
        CandleError::LengthMismatch {
            column,
            expected,
            actual,
        } => PyValueError::new_err(format!(
            "{column}: length {actual} does not match expected {expected}"
        )),
        CandleError::InvalidSignal {
            column,
            bar,
            reason,
        } => PyValueError::new_err(format!("{column}[{bar}]: {reason}")),
        CandleError::Exit(e) => PyValueError::new_err(e.to_string()),
        CandleError::IntrabarSource { bar, reason } => {
            PyValueError::new_err(format!("intrabar[{bar}]: {reason}"))
        }
    }
}

fn contiguous_f64<'a>(argument: &str, array: &'a PyReadonlyArray1<'_, f64>) -> PyResult<&'a [f64]> {
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

fn require_float64(name: &str, obj: &Bound<'_, PyAny>) -> PyResult<()> {
    let ds = dtype_name(obj)?;
    if ds != "float64" {
        return Err(PyValueError::new_err(format!(
            "{name}: array must be C-contiguous float64"
        )));
    }
    Ok(())
}

fn require_int8(name: &str, obj: &Bound<'_, PyAny>) -> PyResult<()> {
    let ds = dtype_name(obj)?;
    if ds != "int8" {
        return Err(PyTypeError::new_err(format!(
            "{name}: expected int8 array, got {ds}"
        )));
    }
    Ok(())
}

fn require_bool(name: &str, obj: &Bound<'_, PyAny>) -> PyResult<()> {
    let ds = dtype_name(obj)?;
    if ds != "bool" && ds != "bool_" {
        return Err(PyValueError::new_err(format!("{name}: array must be bool")));
    }
    Ok(())
}

fn coerce_param_value(py: Python<'_>, value: &Bound<'_, PyAny>) -> PyResult<ParamValue> {
    if value.is_instance_of::<pyo3::types::PyBool>() {
        return Ok(ParamValue::Bool(value.extract()?));
    }
    let builtins = py.import("builtins")?;
    let float_fn = builtins.getattr("float")?;
    let f: f64 = float_fn.call1((value,))?.extract()?;
    if f.is_finite() && f.fract().to_bits() == 0.0_f64.to_bits() {
        let int_fn = builtins.getattr("int")?;
        let i: i64 = int_fn.call1((value,))?.extract()?;
        Ok(ParamValue::Int(i))
    } else {
        Ok(ParamValue::Float(f))
    }
}

fn exit_params_from_mapping(py: Python<'_>, mapping: &Bound<'_, PyAny>) -> PyResult<ExitParams> {
    let dict = mapping.downcast::<PyDict>()?;
    let mut pairs = Vec::with_capacity(dict.len());
    for (key, value) in dict {
        let name: String = key.extract()?;
        pairs.push((name, coerce_param_value(py, &value)?));
    }
    ExitParams::from_pairs(pairs.iter().map(|(name, value)| (name.as_str(), *value)))
        .map_err(|e| PyValueError::new_err(e.to_string()))
}

fn sizing_from_mapping(
    py: Python<'_>,
    mapping: &Bound<'_, PyAny>,
    sizing_point_value: f64,
) -> PyResult<Sizing> {
    let dict = mapping.downcast::<PyDict>()?;
    let kind: String = dict
        .get_item("type")?
        .ok_or_else(|| PyValueError::new_err("sizing.type is required"))?
        .extract()?;
    let scale_key = dict
        .get_item("scale_by_signal_strength")?
        .ok_or_else(|| PyValueError::new_err("sizing.scale_by_signal_strength is required"))?;
    let scale_by_strength: bool = scale_key.extract()?;

    let float_field = |name: &str| -> PyResult<f64> {
        let value = dict
            .get_item(name)?
            .ok_or_else(|| PyValueError::new_err(format!("sizing.{name} is required")))?;
        py.import("builtins")?
            .getattr("float")?
            .call1((value,))?
            .extract()
    };
    let int_field = |name: &str| -> PyResult<i64> {
        let value = dict
            .get_item(name)?
            .ok_or_else(|| PyValueError::new_err(format!("sizing.{name} is required")))?;
        py.import("builtins")?
            .getattr("int")?
            .call1((value,))?
            .extract()
    };
    let optional_int = |name: &str| -> PyResult<Option<i64>> {
        match dict.get_item(name)? {
            None => Ok(None),
            Some(value) if value.is_none() => Ok(None),
            Some(value) => Ok(Some(
                py.import("builtins")?
                    .getattr("int")?
                    .call1((value,))?
                    .extract()?,
            )),
        }
    };

    let sizing = match kind.as_str() {
        "fixed_quantity" => Sizing::FixedQuantity {
            quantity: float_field("quantity")?,
            scale_by_strength,
        },
        "fixed_safety_margin" => Sizing::FixedSafetyMargin {
            margin_per_contract: float_field("safety_margin_per_contract")?,
            min_contracts: int_field("min_contracts")?,
            max_contracts: optional_int("max_contracts")?,
            scale_by_strength,
        },
        "inverse_volatility" => Sizing::InverseVolatility {
            target_volatility_pct: float_field("target_volatility_pct")?,
            point_value: sizing_point_value,
            min_contracts: int_field("min_contracts")?,
            max_contracts: optional_int("max_contracts")?,
            scale_by_strength,
        },
        other => {
            return Err(PyValueError::new_err(format!(
                "unknown sizing type {other}"
            )))
        }
    };
    sizing.validate().map_err(candle_error)?;
    Ok(sizing)
}

fn exit_reason_text(code: i64) -> Option<&'static str> {
    if code < 0 {
        return None;
    }
    if (0..=10).contains(&code) {
        return ExitRuleId::REGISTRY_ORDER
            .iter()
            .find(|id| (**id as u8 as i64) == code)
            .map(|id| id.as_str());
    }
    Some(match code {
        11 => "SIGNAL",
        12 => "END_OF_DAY",
        13 => "FORCE_CLOSE",
        14 => "STOP_LOSS",
        15 => "TAKE_PROFIT",
        _ => return None,
    })
}

fn rule_state_to_dict<'py>(
    py: Python<'py>,
    state: &RuleState,
    side: Side,
    rules: &ExitRuleSet,
) -> PyResult<Bound<'py, PyDict>> {
    let out = PyDict::new(py);
    for rule in rules.enabled() {
        let rule_dict = match rule {
            ExitRuleId::Trailing => {
                let Some(extreme) = state.trailing_extreme else {
                    continue;
                };
                let d = PyDict::new(py);
                d.set_item("extreme", extreme)?;
                d
            }
            ExitRuleId::Chandelier => {
                let Some(peak) = state.chandelier_peak else {
                    continue;
                };
                let d = PyDict::new(py);
                d.set_item("peak", peak)?;
                d
            }
            ExitRuleId::Breakeven => {
                if !state.breakeven_armed {
                    continue;
                }
                let d = PyDict::new(py);
                d.set_item("armed", true)?;
                d
            }
            ExitRuleId::ParabolicSar => {
                let Some(psar) = state.psar else {
                    continue;
                };
                let d = PyDict::new(py);
                d.set_item("sar", psar.sar)?;
                d.set_item("ep", psar.ep)?;
                d.set_item("af", psar.af)?;
                match side {
                    Side::Long => {
                        d.set_item("prior_low", psar.prior)?;
                        if let Some(pp) = psar.prior_prior {
                            d.set_item("prior_prior_low", pp)?;
                        }
                    }
                    Side::Short => {
                        d.set_item("prior_high", psar.prior)?;
                        if let Some(pp) = psar.prior_prior {
                            d.set_item("prior_prior_high", pp)?;
                        }
                    }
                }
                d
            }
            ExitRuleId::ProfitTargetRatchet => {
                let Some(level) = state.ratchet else {
                    continue;
                };
                let d = PyDict::new(py);
                d.set_item("armed", true)?;
                d.set_item("ratchet", level)?;
                d
            }
            ExitRuleId::TimeStop => {
                if state.time_stop_bars <= 0 {
                    continue;
                }
                let d = PyDict::new(py);
                d.set_item("bars", state.time_stop_bars)?;
                d
            }
            _ => continue,
        };
        out.set_item(rule.as_str(), rule_dict)?;
    }
    Ok(out)
}

#[pyfunction]
fn required_columns(exit_params: &Bound<'_, PyAny>) -> PyResult<Vec<String>> {
    let params = exit_params_from_mapping(exit_params.py(), exit_params)?;
    Ok(ExitRuleSet::new(params).required_columns())
}

#[pyfunction]
fn enabled_rules(exit_params: &Bound<'_, PyAny>) -> PyResult<Vec<String>> {
    let params = exit_params_from_mapping(exit_params.py(), exit_params)?;
    Ok(ExitRuleSet::new(params)
        .enabled()
        .iter()
        .map(|id| id.as_str().to_string())
        .collect())
}

#[pyfunction]
#[pyo3(signature = (sizing, *, point_value, strength, price, capital, volatility=None))]
fn size_entry(
    py: Python<'_>,
    sizing: &Bound<'_, PyAny>,
    point_value: f64,
    strength: f64,
    price: f64,
    capital: f64,
    volatility: Option<f64>,
) -> PyResult<Option<f64>> {
    let model = sizing_from_mapping(py, sizing, point_value)?;
    Ok(model.size(strength, price, capital, volatility))
}

#[pyfunction]
#[pyo3(signature = (sizing, *, point_value, price, capital))]
fn max_position(
    py: Python<'_>,
    sizing: &Bound<'_, PyAny>,
    point_value: f64,
    price: f64,
    capital: f64,
) -> PyResult<Option<f64>> {
    let model = sizing_from_mapping(py, sizing, point_value)?;
    Ok(model.max_position(price, capital))
}

fn extract_f64<'py>(
    name: &'static str,
    obj: &Bound<'py, PyAny>,
) -> PyResult<PyReadonlyArray1<'py, f64>> {
    require_float64(name, obj)?;
    obj.extract()
}

#[allow(clippy::too_many_arguments)]
#[pyfunction(name = "run_candle")]
#[pyo3(signature = (
    *,
    time_us,
    open=None,
    high=None,
    low=None,
    close=None,
    entry,
    exit_long,
    exit_short,
    strength,
    bar_index=None,
    volatility=None,
    tradable=None,
    columns,
    initial_capital,
    point_value,
    costs=None,
    sizing,
    sizing_point_value,
    holding_period_bars=None,
    exit_params,
    day_trade_us=None,
    force_close_at_end,
    stop_price=None,
    target_price=None,
    intrabar=None,
    strategy_callback=None,
    exit_screen_callback=None,
    exit_tick_callback=None,
))]
fn py_run_candle<'py>(
    py: Python<'py>,
    time_us: PyReadonlyArray1<'py, i64>,
    open: Option<&Bound<'py, PyAny>>,
    high: Option<&Bound<'py, PyAny>>,
    low: Option<&Bound<'py, PyAny>>,
    close: Option<&Bound<'py, PyAny>>,
    entry: &Bound<'py, PyAny>,
    exit_long: &Bound<'py, PyAny>,
    exit_short: &Bound<'py, PyAny>,
    strength: &Bound<'py, PyAny>,
    bar_index: Option<PyReadonlyArray1<'py, i64>>,
    volatility: Option<&Bound<'py, PyAny>>,
    tradable: Option<&Bound<'py, PyAny>>,
    columns: &Bound<'py, PyAny>,
    initial_capital: f64,
    point_value: f64,
    costs: Option<(f64, f64)>,
    sizing: &Bound<'py, PyAny>,
    sizing_point_value: f64,
    holding_period_bars: Option<i64>,
    exit_params: &Bound<'py, PyAny>,
    day_trade_us: Option<(i64, i64, i64)>,
    force_close_at_end: bool,
    stop_price: Option<&Bound<'py, PyAny>>,
    target_price: Option<&Bound<'py, PyAny>>,
    intrabar: Option<&Bound<'py, PyAny>>,
    strategy_callback: Option<&Bound<'py, PyAny>>,
    exit_screen_callback: Option<&Bound<'py, PyAny>>,
    exit_tick_callback: Option<&Bound<'py, PyAny>>,
) -> PyResult<Bound<'py, PyDict>> {
    require_int8("entry", entry)?;
    let entry_arr: PyReadonlyArray1<'py, i8> = entry.extract()?;
    let time_us = contiguous_i64("time_us", &time_us)?;
    let entry = contiguous_i8("entry", &entry_arr)?;
    require_bool("exit_long", exit_long)?;
    require_bool("exit_short", exit_short)?;
    let exit_long_arr: PyReadonlyArray1<'py, bool> = exit_long.extract()?;
    let exit_short_arr: PyReadonlyArray1<'py, bool> = exit_short.extract()?;
    let exit_long = exit_long_arr
        .as_slice()
        .map_err(|_| PyValueError::new_err("exit_long: array must be C-contiguous bool"))?;
    let exit_short = exit_short_arr
        .as_slice()
        .map_err(|_| PyValueError::new_err("exit_short: array must be C-contiguous bool"))?;
    require_float64("strength", strength)?;
    let strength_arr: PyReadonlyArray1<'py, f64> = strength.extract()?;
    let strength = contiguous_f64("strength", &strength_arr)?;

    let open_arr = open.map(|obj| extract_f64("open", obj)).transpose()?;
    let high_arr = high.map(|obj| extract_f64("high", obj)).transpose()?;
    let low_arr = low.map(|obj| extract_f64("low", obj)).transpose()?;
    let close_arr = close.map(|obj| extract_f64("close", obj)).transpose()?;
    let volatility_arr = volatility
        .map(|obj| extract_f64("volatility", obj))
        .transpose()?;
    let open = open_arr
        .as_ref()
        .map(|a| contiguous_f64("open", a))
        .transpose()?;
    let high = high_arr
        .as_ref()
        .map(|a| contiguous_f64("high", a))
        .transpose()?;
    let low = low_arr
        .as_ref()
        .map(|a| contiguous_f64("low", a))
        .transpose()?;
    let close = close_arr
        .as_ref()
        .map(|a| contiguous_f64("close", a))
        .transpose()?;
    let bar_index = bar_index
        .as_ref()
        .map(|a| contiguous_i64("bar_index", a))
        .transpose()?;
    let volatility = volatility_arr
        .as_ref()
        .map(|a| contiguous_f64("volatility", a))
        .transpose()?;
    let stop_arr = stop_price
        .map(|obj| extract_f64("stop_price", obj))
        .transpose()?;
    let target_arr = target_price
        .map(|obj| extract_f64("target_price", obj))
        .transpose()?;
    let stop_price = stop_arr
        .as_ref()
        .map(|a| contiguous_f64("stop_price", a))
        .transpose()?;
    let target_price = target_arr
        .as_ref()
        .map(|a| contiguous_f64("target_price", a))
        .transpose()?;
    if stop_price.is_some() != target_price.is_some() {
        return Err(PyValueError::new_err(
            "stop_price and target_price must be passed together",
        ));
    }
    let source = intrabar.map(|callable| PyIntrabar {
        callable,
        error: RefCell::new(None),
    });

    let tradable_arr = tradable
        .map(|arr| {
            require_bool("tradable", arr)?;
            arr.extract::<PyReadonlyArray1<'py, bool>>()
        })
        .transpose()?;
    let tradable_slice = tradable_arr
        .as_ref()
        .map(|view| {
            view.as_slice()
                .map_err(|_| PyValueError::new_err("tradable: array must be C-contiguous bool"))
        })
        .transpose()?;

    let col_dict = columns.downcast::<PyDict>()?;
    let mut named: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    for (key, value) in col_dict {
        let name: String = key.extract()?;
        require_float64(&name, &value)?;
        let arr: PyReadonlyArray1<'py, f64> = value.extract()?;
        let values = contiguous_f64(name.as_str(), &arr)?.to_vec();
        named.insert(name, values);
    }

    let exit_params_rust = exit_params_from_mapping(py, exit_params)?;
    let sizing_rust = sizing_from_mapping(py, sizing, sizing_point_value)?;
    let config = CandleConfig {
        initial_capital,
        point_value,
        costs: costs.map(|(per_contract, bps)| Costs { per_contract, bps }),
        sizing: sizing_rust,
        holding_period_bars,
        exit_params: exit_params_rust,
        day_trade: day_trade_us.map(|(entry_start_us, entry_end_us, force_close_us)| {
            DayTradeWindow {
                entry_start_us,
                entry_end_us,
                force_close_us,
            }
        }),
        force_close_at_end,
    };

    if exit_screen_callback.is_some() != exit_tick_callback.is_some() {
        return Err(PyValueError::new_err(
            "exit_screen_callback and exit_tick_callback must be passed together",
        ));
    }

    // Host code can retain and mutate its numpy arrays; callback runs read owned copies.
    let owns_buffers = strategy_callback.is_some() || exit_screen_callback.is_some();
    let owned_time_us = owns_buffers.then(|| time_us.to_vec());
    let time_us = owned_time_us.as_deref().unwrap_or(time_us);
    let owned_entry = owns_buffers.then(|| entry.to_vec());
    let entry = owned_entry.as_deref().unwrap_or(entry);
    let owned_exit_long = owns_buffers.then(|| exit_long.to_vec());
    let exit_long = owned_exit_long.as_deref().unwrap_or(exit_long);
    let owned_exit_short = owns_buffers.then(|| exit_short.to_vec());
    let exit_short = owned_exit_short.as_deref().unwrap_or(exit_short);
    let owned_strength = owns_buffers.then(|| strength.to_vec());
    let strength = owned_strength.as_deref().unwrap_or(strength);
    let owned_open = if owns_buffers {
        open.map(<[_]>::to_vec)
    } else {
        None
    };
    let open = owned_open.as_deref().or(open);
    let owned_high = if owns_buffers {
        high.map(<[_]>::to_vec)
    } else {
        None
    };
    let high = owned_high.as_deref().or(high);
    let owned_low = if owns_buffers {
        low.map(<[_]>::to_vec)
    } else {
        None
    };
    let low = owned_low.as_deref().or(low);
    let owned_close = if owns_buffers {
        close.map(<[_]>::to_vec)
    } else {
        None
    };
    let close = owned_close.as_deref().or(close);
    let owned_bar_index = if owns_buffers {
        bar_index.map(<[_]>::to_vec)
    } else {
        None
    };
    let bar_index = owned_bar_index.as_deref().or(bar_index);
    let owned_volatility = if owns_buffers {
        volatility.map(<[_]>::to_vec)
    } else {
        None
    };
    let volatility = owned_volatility.as_deref().or(volatility);
    let owned_tradable_slice = if owns_buffers {
        tradable_slice.map(<[_]>::to_vec)
    } else {
        None
    };
    let tradable_slice = owned_tradable_slice.as_deref().or(tradable_slice);
    let owned_stop_price = if owns_buffers {
        stop_price.map(<[_]>::to_vec)
    } else {
        None
    };
    let stop_price = owned_stop_price.as_deref().or(stop_price);
    let owned_target_price = if owns_buffers {
        target_price.map(<[_]>::to_vec)
    } else {
        None
    };
    let target_price = owned_target_price.as_deref().or(target_price);
    let protective = match (stop_price, target_price) {
        (Some(stop_price), Some(target_price)) => Some(ProtectiveColumns {
            stop_price,
            target_price,
        }),
        _ => None,
    };
    let lookup = |name: &str| named.get(name).map(|v| v.as_slice());
    let inputs = CandleInputs {
        time_us,
        open,
        high,
        low,
        close,
        signals: SignalColumns {
            entry,
            exit_long,
            exit_short,
            strength,
            bar_index,
        },
        volatility,
        tradable: tradable_slice,
        columns: &lookup,
        protective,
        intrabar: source.as_ref().map(|source| source as &dyn IntrabarSource),
    };

    let mut strategy = strategy_callback.map(|callback| {
        move |bar: usize, positions: &[PositionSnapshot]| -> Result<BarSignals, StrategyRunError> {
            let snapshot = position_tuples(py, positions)?;
            let returned = callback.call1((bar, snapshot))?;
            strategy_decision(&returned)
        }
    });
    let mut screen = exit_screen_callback.map(|callback| {
        move |bar: usize, positions: &[PositionSnapshot]| -> Result<bool, StrategyRunError> {
            let snapshot = position_tuples(py, positions)?;
            let returned = callback.call1((bar, snapshot))?;
            exit_flag(&returned, "exit_screen_callback")
        }
    });
    let mut tick = exit_tick_callback.map(|callback| {
        move |bar: usize,
              tick: usize,
              time_us: i64,
              price: f64,
              positions: &[PositionSnapshot]|
              -> Result<bool, StrategyRunError> {
            let snapshot = position_tuples(py, positions)?;
            let returned = callback.call1((bar, tick, time_us, price, snapshot))?;
            exit_flag(&returned, "exit_tick_callback")
        }
    });
    let exit = match (screen.as_mut(), tick.as_mut()) {
        (Some(screen), Some(tick)) => Some(ExitCallbacks { screen, tick }),
        _ => None,
    };
    let result = run_candle_with_callbacks(
        &inputs,
        &config,
        RuntimeCallbacks {
            strategy: strategy
                .as_mut()
                .map(|closure| closure as &mut StrategyFn<'_, StrategyRunError>),
            exit,
        },
    );
    let run = result.map_err(|err| {
        if let Some(raised) = source
            .as_ref()
            .and_then(|source| source.error.borrow_mut().take())
        {
            return raised;
        }
        match err {
            StrategyRunError::Kernel(err) => candle_error(err),
            StrategyRunError::Python(err) => err,
        }
    })?;
    let ledger = &run.trades;
    let trace = &run.trace;
    let rejected = &run.rejected;

    let out = PyDict::new(py);
    out.set_item(
        "entry_bar",
        PyArray1::from_vec(py, ledger.entry_bar.clone()),
    )?;
    out.set_item("exit_bar", PyArray1::from_vec(py, ledger.exit_bar.clone()))?;
    out.set_item("side", PyArray1::from_vec(py, ledger.side.clone()))?;
    out.set_item("quantity", PyArray1::from_vec(py, ledger.quantity.clone()))?;
    out.set_item(
        "entry_price",
        PyArray1::from_vec(py, ledger.entry_price.clone()),
    )?;
    out.set_item(
        "exit_price",
        PyArray1::from_vec(py, ledger.exit_price.clone()),
    )?;
    out.set_item(
        "commission",
        PyArray1::from_vec(py, ledger.commission.clone()),
    )?;
    out.set_item("pnl", PyArray1::from_vec(py, ledger.pnl.clone()))?;
    out.set_item(
        "exit_reason",
        PyArray1::from_vec(py, ledger.exit_reason.clone()),
    )?;

    let texts: Vec<Option<&str>> = ledger
        .exit_reason
        .iter()
        .map(|&code| exit_reason_text(code))
        .collect();
    out.set_item("exit_reason_text", texts)?;
    out.set_item(
        "exit_time_us",
        PyArray1::from_vec(py, ledger.exit_time_us.clone()),
    )?;

    out.set_item(
        "exit_offsets",
        PyArray1::from_vec(py, trace.exit_offsets.clone()),
    )?;
    out.set_item(
        "decision_exit_reason",
        PyArray1::from_vec(py, trace.exit_reason.clone()),
    )?;
    out.set_item("entry", PyArray1::from_vec(py, trace.entry.clone()))?;
    out.set_item(
        "entry_strength",
        PyArray1::from_vec(py, trace.entry_strength.clone()),
    )?;
    out.set_item("rejected_bar", PyArray1::from_vec(py, rejected.bar.clone()))?;
    out.set_item(
        "rejected_side",
        PyArray1::from_vec(py, rejected.side.clone()),
    )?;
    out.set_item(
        "rejected_fill_price",
        PyArray1::from_vec(py, rejected.fill_price.clone()),
    )?;
    out.set_item(
        "rejected_stop_price",
        PyArray1::from_vec(py, rejected.stop_price.clone()),
    )?;
    out.set_item(
        "rejected_target_price",
        PyArray1::from_vec(py, rejected.target_price.clone()),
    )?;

    Ok(out)
}

fn position_tuples<'py>(
    py: Python<'py>,
    positions: &[PositionSnapshot],
) -> PyResult<Bound<'py, PyTuple>> {
    let positions: Vec<_> = positions
        .iter()
        .map(|position| {
            (
                position.key.0,
                if matches!(position.side, Side::Long) {
                    1_i8
                } else {
                    -1_i8
                },
                position.entry_bar,
                position.entry_price,
                position.quantity,
            )
        })
        .collect();
    PyTuple::new(py, positions)
}

/// `(entry, exit_long, exit_short, strength)`, optionally followed by the `(stop_price,
/// target_price)` of an entry queued on this bar. `NaN` means no level.
fn strategy_decision(returned: &Bound<'_, PyAny>) -> Result<BarSignals, StrategyRunError> {
    let tuple = returned.downcast::<PyTuple>().map_err(|_| {
        StrategyRunError::Python(PyTypeError::new_err(
            "strategy_callback must return (entry, exit_long, exit_short, strength[, stop_price, target_price])",
        ))
    })?;
    if tuple.len() != 4 && tuple.len() != 6 {
        return Err(StrategyRunError::Python(PyTypeError::new_err(
            "strategy_callback must return four or six values",
        )));
    }
    let entry = tuple.get_item(0)?;
    let long = tuple.get_item(1)?;
    let short = tuple.get_item(2)?;
    if entry.is_instance_of::<PyBool>()
        || !long.is_instance_of::<PyBool>()
        || !short.is_instance_of::<PyBool>()
    {
        return Err(StrategyRunError::Python(PyTypeError::new_err(
            "strategy_callback requires integer entry and boolean exit flags",
        )));
    }
    let levels = if tuple.len() == 6 {
        let stop: f64 = tuple.get_item(4)?.extract()?;
        let target: f64 = tuple.get_item(5)?.extract()?;
        Some((stop, target))
    } else {
        None
    };
    Ok(BarSignals {
        entry: entry.extract()?,
        exit_long: long.extract()?,
        exit_short: short.extract()?,
        strength: tuple.get_item(3)?.extract()?,
        levels,
    })
}

fn exit_flag(returned: &Bound<'_, PyAny>, name: &str) -> Result<bool, StrategyRunError> {
    if !returned.is_instance_of::<PyBool>() {
        return Err(StrategyRunError::Python(PyTypeError::new_err(format!(
            "{name} must return a bool"
        ))));
    }
    Ok(returned.extract::<bool>()?)
}

/// The Python `intrabar` callable as an [`IntrabarSource`]. It returns `(time_us, price)`
/// arrays for a bar. A raised exception is kept here and re-raised by the caller.
struct PyIntrabar<'py> {
    callable: &'py Bound<'py, PyAny>,
    error: RefCell<Option<PyErr>>,
}

impl IntrabarSource for PyIntrabar<'_> {
    fn prices(&self, bar: usize) -> Result<IntrabarPrices, String> {
        let prices = self.callable.call1((bar,)).and_then(|pair| {
            let (time_us, price): (PyReadonlyArray1<'_, i64>, PyReadonlyArray1<'_, f64>) =
                pair.extract()?;
            Ok(IntrabarPrices {
                time_us: time_us.as_array().to_vec(),
                price: price.as_array().to_vec(),
            })
        });
        prices.map_err(|err| {
            *self.error.borrow_mut() = Some(err);
            "intrabar callable raised an exception".to_string()
        })
    }
}

struct IdInterner {
    ids: BTreeMap<String, PositionKey>,
    sides: BTreeMap<PositionKey, Side>,
    next: u64,
}

impl IdInterner {
    fn new() -> Self {
        Self {
            ids: BTreeMap::new(),
            sides: BTreeMap::new(),
            next: 0,
        }
    }

    fn key_for(&mut self, trade_id: &str, side: Side) -> PositionKey {
        if let Some(key) = self.ids.get(trade_id) {
            return *key;
        }
        let key = PositionKey(self.next);
        self.next += 1;
        self.ids.insert(trade_id.to_string(), key);
        self.sides.insert(key, side);
        key
    }

    fn id_for(&self, key: PositionKey) -> Option<&str> {
        self.ids
            .iter()
            .find_map(|(id, k)| (*k == key).then_some(id.as_str()))
    }

    fn side_for(&self, key: PositionKey) -> Option<Side> {
        self.sides.get(&key).copied()
    }

    fn prune(&mut self, active: &BTreeSet<PositionKey>) {
        self.ids.retain(|_, key| active.contains(key));
        self.sides.retain(|key, _| active.contains(key));
    }
}

#[pyclass(name = "DecisionStep", module = "q_core.engine")]
struct PyDecisionStep {
    step: DecisionStep,
    rules: ExitRuleSet,
    interner: IdInterner,
}

#[pymethods]
impl PyDecisionStep {
    #[new]
    #[pyo3(signature = (*, exit_params))]
    fn new(exit_params: &Bound<'_, PyAny>) -> PyResult<Self> {
        let params = exit_params_from_mapping(exit_params.py(), exit_params)?;
        let rules = ExitRuleSet::new(params.clone());
        Ok(Self {
            step: DecisionStep::new(params),
            rules,
            interner: IdInterner::new(),
        })
    }

    #[allow(clippy::too_many_arguments)]
    #[pyo3(signature = (
        *,
        bar,
        trades,
        close=None,
        high=None,
        low=None,
        entry,
        exit_long,
        exit_short,
        strength,
        bar_index=None,
        columns,
        holding_period_bars=None,
    ))]
    fn decide<'py>(
        &mut self,
        py: Python<'py>,
        bar: usize,
        trades: &Bound<'py, PyAny>,
        close: Option<&Bound<'py, PyAny>>,
        high: Option<&Bound<'py, PyAny>>,
        low: Option<&Bound<'py, PyAny>>,
        entry: &Bound<'py, PyAny>,
        exit_long: &Bound<'py, PyAny>,
        exit_short: &Bound<'py, PyAny>,
        strength: &Bound<'py, PyAny>,
        bar_index: Option<PyReadonlyArray1<'py, i64>>,
        columns: &Bound<'py, PyAny>,
        holding_period_bars: Option<i64>,
    ) -> PyResult<Bound<'py, PyTuple>> {
        require_int8("entry", entry)?;
        let entry_arr: PyReadonlyArray1<'py, i8> = entry.extract()?;
        let entry = contiguous_i8("entry", &entry_arr)?;
        require_bool("exit_long", exit_long)?;
        require_bool("exit_short", exit_short)?;
        let exit_long_arr: PyReadonlyArray1<'py, bool> = exit_long.extract()?;
        let exit_short_arr: PyReadonlyArray1<'py, bool> = exit_short.extract()?;
        let exit_long = exit_long_arr
            .as_slice()
            .map_err(|_| PyValueError::new_err("exit_long: array must be C-contiguous bool"))?;
        let exit_short = exit_short_arr
            .as_slice()
            .map_err(|_| PyValueError::new_err("exit_short: array must be C-contiguous bool"))?;
        require_float64("strength", strength)?;
        let strength_arr: PyReadonlyArray1<'py, f64> = strength.extract()?;
        let strength = contiguous_f64("strength", &strength_arr)?;
        let bar_index = bar_index
            .as_ref()
            .map(|a| contiguous_i64("bar_index", a))
            .transpose()?;

        let close_arr = close
            .map(|obj| extract_f64("close", obj))
            .transpose()?
            .ok_or_else(|| PyValueError::new_err("close is required for exit-rule evaluation"))?;
        let high_arr = high.map(|obj| extract_f64("high", obj)).transpose()?;
        let low_arr = low.map(|obj| extract_f64("low", obj)).transpose()?;
        let close = contiguous_f64("close", &close_arr)?;
        let high = high_arr
            .as_ref()
            .map(|a| contiguous_f64("high", a))
            .transpose()?;
        let low = low_arr
            .as_ref()
            .map(|a| contiguous_f64("low", a))
            .transpose()?;

        let col_dict = columns.downcast::<PyDict>()?;
        let mut named: BTreeMap<String, Vec<f64>> = BTreeMap::new();
        for (key, value) in col_dict {
            let name: String = key.extract()?;
            require_float64(&name, &value)?;
            let arr: PyReadonlyArray1<'py, f64> = value.extract()?;
            let values = contiguous_f64(name.as_str(), &arr)?.to_vec();
            named.insert(name, values);
        }

        let exit_inputs = ExitInputs::from_slices(
            close,
            high,
            low,
            &|name| named.get(name).map(|v| v.as_slice()),
            &self.rules,
        )
        .map_err(|e| PyValueError::new_err(e.to_string()))?;

        let signals = SignalColumns {
            entry,
            exit_long,
            exit_short,
            strength,
            bar_index,
        };

        let trade_list = trades.downcast::<PyList>()?;
        let mut views = Vec::with_capacity(trade_list.len());
        let mut active = BTreeSet::new();
        for item in trade_list {
            let tuple = item.downcast::<PyTuple>()?;
            if tuple.len() != 4 {
                return Err(PyValueError::new_err(
                    "each trade must be (id, side, entry_price, entry_bar)",
                ));
            }
            let trade_id: String = tuple.get_item(0)?.extract()?;
            let side_code: i64 = tuple.get_item(1)?.extract()?;
            let entry_price: f64 = tuple.get_item(2)?.extract()?;
            let entry_bar: Option<i64> = tuple.get_item(3)?.extract()?;
            let side = match side_code {
                1 => Side::Long,
                -1 => Side::Short,
                other => {
                    return Err(PyValueError::new_err(format!(
                        "trade side {other} is not 1 or -1"
                    )));
                }
            };
            let key = self.interner.key_for(&trade_id, side);
            active.insert(key);
            views.push(TradeView {
                key,
                side,
                entry_price,
                entry_bar: entry_bar.map(|b| b as usize),
            });
        }
        self.interner.prune(&active);

        let decision = self
            .step
            .decide(bar, &views, &signals, &exit_inputs, holding_period_bars);

        let exits = PyList::empty(py);
        for exit in &decision.exits {
            let trade_id = self
                .interner
                .id_for(exit.key)
                .ok_or_else(|| PyValueError::new_err("unknown trade key in exit"))?;
            let reason = exit.rule.map(|rule| rule.as_str().to_string());
            exits.append((trade_id, reason))?;
        }

        let entry_out = match decision.entry {
            Some(QueuedEntry { side, strength }) => {
                let code = match side {
                    Side::Long => 1,
                    Side::Short => -1,
                };
                (code, strength).into_bound_py_any(py)?
            }
            None => py.None().into_bound_py_any(py)?,
        };

        PyTuple::new(py, [exits.into_any(), entry_out])
    }

    fn state<'py>(&self, py: Python<'py>, trade_id: &str) -> PyResult<Option<Bound<'py, PyDict>>> {
        let key = match self.interner.ids.get(trade_id) {
            Some(key) => *key,
            None => return Ok(None),
        };
        let Some(state) = self.step.state(key) else {
            return Ok(None);
        };
        let side = self
            .interner
            .side_for(key)
            .ok_or_else(|| PyValueError::new_err("missing side for trade id"))?;
        Ok(Some(rule_state_to_dict(py, state, side, &self.rules)?))
    }
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

fn parse_tick_sizing(sizing: &Bound<'_, PyAny>) -> PyResult<TickSizing> {
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

fn extract_tick_f64<'py>(
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

fn extract_tick_i64<'py>(
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

fn extract_tick_i8<'py>(
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
    let bid_a = extract_tick_f64("bid", &bid)?;
    let ask_a = extract_tick_f64("ask", &ask)?;
    let direction_a = extract_tick_i8("direction", &direction)?;
    let sl_a = extract_tick_f64("sl_points", &sl_points)?;
    let tp_a = extract_tick_f64("tp_points", &tp_points)?;

    let bid_s = contiguous_f64("bid", &bid_a)?;
    let ask_s = contiguous_f64("ask", &ask_a)?;
    let direction_s = contiguous_i8("direction", &direction_a)?;
    let sl_s = contiguous_f64("sl_points", &sl_a)?;
    let tp_s = contiguous_f64("tp_points", &tp_a)?;

    let sizing = parse_tick_sizing(&sizing)?;
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
    let time_a = extract_tick_i64("time_msc", &time_msc)?;
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
    let time_a = extract_tick_i64("time_msc", &time_msc)?;
    let bid_a = extract_tick_f64("bid", &bid)?;
    let ask_a = extract_tick_f64("ask", &ask)?;
    let last_a = extract_tick_f64("last", &last)?;
    let volume_a = extract_tick_f64("volume", &volume)?;

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
    let series_a = extract_tick_f64("series", &series)?;
    let tick_end_a = extract_tick_i64("tick_end", &tick_end)?;
    let series_s = contiguous_f64("series", &series_a)?;
    let tick_end_s = contiguous_i64("tick_end", &tick_end_a)?;
    let out = sample_at_bar_ends_rs(series_s, tick_end_s);
    Ok(PyArray1::from_vec(py, out))
}

pub(crate) fn register(parent: &Bound<'_, PyModule>) -> PyResult<()> {
    let m = PyModule::new(parent.py(), "engine")?;
    m.add_function(wrap_pyfunction!(required_columns, &m)?)?;
    m.add_function(wrap_pyfunction!(enabled_rules, &m)?)?;
    m.add_function(wrap_pyfunction!(size_entry, &m)?)?;
    m.add_function(wrap_pyfunction!(max_position, &m)?)?;
    m.add_function(wrap_pyfunction!(py_run_candle, &m)?)?;
    m.add_class::<PyDecisionStep>()?;
    m.add_function(wrap_pyfunction!(tick_simulate, &m)?)?;
    m.add_function(wrap_pyfunction!(tick_day_bounds, &m)?)?;
    m.add_function(wrap_pyfunction!(tick_bars, &m)?)?;
    m.add_function(wrap_pyfunction!(resolve_bar_ms, &m)?)?;
    m.add_function(wrap_pyfunction!(sample_at_bar_ends, &m)?)?;
    m.add("PROTECTIVE_ORDERS", true)?;
    parent.add_submodule(&m)?;
    parent
        .py()
        .import("sys")?
        .getattr("modules")?
        .set_item("q_core.engine", &m)?;
    Ok(())
}
