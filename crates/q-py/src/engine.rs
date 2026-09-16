//! Python projection of the candle kernel: `run_candle`, sizing helpers, and `DecisionStep`.

use std::collections::{BTreeMap, BTreeSet};

use numpy::{PyArray1, PyReadonlyArray1};
use pyo3::exceptions::{PyTypeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList, PyModule, PyTuple};
use pyo3::Bound;
use pyo3::IntoPyObjectExt;
use q_engine::{
    run_candle as run_candle_kernel, CandleConfig, CandleError, CandleInputs, Costs,
    DayTradeWindow, DecisionStep, ExitInputs, ExitParams, ExitRuleId, ExitRuleSet, ParamValue,
    PositionKey, QueuedEntry, RuleState, Side, SignalColumns, Sizing, TradeView,
};

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
    };

    let run = run_candle_kernel(&inputs, &config).map_err(candle_error)?;
    let ledger = &run.trades;
    let trace = &run.trace;

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

    Ok(out)
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

pub(crate) fn register(parent: &Bound<'_, PyModule>) -> PyResult<()> {
    let m = PyModule::new(parent.py(), "engine")?;
    m.add_function(wrap_pyfunction!(required_columns, &m)?)?;
    m.add_function(wrap_pyfunction!(enabled_rules, &m)?)?;
    m.add_function(wrap_pyfunction!(size_entry, &m)?)?;
    m.add_function(wrap_pyfunction!(max_position, &m)?)?;
    m.add_function(wrap_pyfunction!(py_run_candle, &m)?)?;
    m.add_class::<PyDecisionStep>()?;
    parent.add_submodule(&m)?;
    parent
        .py()
        .import("sys")?
        .getattr("modules")?
        .set_item("q_core.engine", &m)?;
    Ok(())
}
