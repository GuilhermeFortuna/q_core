#![forbid(unsafe_code)]

//! `decision_step` reference gate: what one closed bar queues, and the quantity the forward
//! evaluator would request for it, must match section D of the backend loop bar for bar.

use q_engine::{
    Decision, DecisionStep, ExitInputs, ExitParams, ExitReason, ExitRuleSet, ParamValue,
    PositionKey, Side, SignalColumns, Sizing, TradeView,
};
use q_parity::compare::{compare_f64, Mismatch, MismatchKind};
use q_parity::fixture::{load_family, Column, ColumnData, FixtureFile};
use q_parity::gate::{check_accounting, parse_pending, GateFailure, GateReport};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const BACKEND_REV: &str = include_str!("../../../BACKEND_REV");

const BOUND_SCENARIOS: &[&str] = &[
    "d01_no_trade_inverse_volatility",
    "d02_long_open_scripted_exit",
    "d03_short_open_scripted_exit",
    "d04_trailing_atr_stop_long",
    "d05_psar_time_stop_short",
    "d06_holding_period_on_grid",
    "d07_holding_period_off_grid",
];

/// Trace columns the exporter writes as int64.
const TRACE_INTS: &[&str] = &["entry", "exit_offsets", "exit_reason"];
/// Trace columns the exporter writes as float64.
const TRACE_FLOATS: &[&str] = &["entry_strength", "requested_quantity"];

// Q-024's decision column names, from `tools/reference/families/scripted_strategy.py`.
const ENTRY_COLUMN: &str = "entry_signal";
const EXIT_LONG_COLUMN: &str = "exit_long_signal";
const EXIT_SHORT_COLUMN: &str = "exit_short_signal";
const STRENGTH_COLUMN: &str = "signal_strength";
const BAR_INDEX_COLUMN: &str = "bar_index";

fn fixtures_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/reference")
}

// ---------------------------------------------------------------------------
// Column decoding
// ---------------------------------------------------------------------------

fn float_column(value: &Value, name: &str) -> Result<Vec<f64>, String> {
    match Column::from_json(value)
        .map_err(|e| format!("{name}: {e}"))?
        .data
    {
        ColumnData::Float64(values) => Ok(values),
        ColumnData::Int64(_) => Err(format!("{name}: expected float64")),
    }
}

fn int_column(value: &Value, name: &str) -> Result<Vec<i64>, String> {
    match Column::from_json(value)
        .map_err(|e| format!("{name}: {e}"))?
        .data
    {
        ColumnData::Int64(values) => Ok(values),
        ColumnData::Float64(_) => Err(format!("{name}: expected int64")),
    }
}

/// The exporter stores both exit columns as 0/1 int64; anything else is a contract break.
fn bool_column(value: &Value, name: &str) -> Result<Vec<bool>, String> {
    int_column(value, name)?
        .into_iter()
        .map(|flag| match flag {
            0 => Ok(false),
            1 => Ok(true),
            other => Err(format!("{name}: {other} is not a 0/1 flag")),
        })
        .collect()
}

/// `entry_signal` is int64 in the fixture and `i8` in [`SignalColumns`].
fn entry_column(value: &Value, name: &str) -> Result<Vec<i8>, String> {
    int_column(value, name)?
        .into_iter()
        .map(|entry| i8::try_from(entry).map_err(|_| format!("{name}: {entry} does not fit in i8")))
        .collect()
}

fn required<'a>(inputs: &'a Map<String, Value>, name: &str) -> Result<&'a Value, String> {
    inputs
        .get(name)
        .ok_or_else(|| format!("missing input column {name}"))
}

fn optional_floats(inputs: &Map<String, Value>, name: &str) -> Result<Option<Vec<f64>>, String> {
    inputs.get(name).map(|v| float_column(v, name)).transpose()
}

// ---------------------------------------------------------------------------
// Config decoding
// ---------------------------------------------------------------------------

fn as_f64(obj: &Map<String, Value>, name: &str) -> Result<f64, String> {
    obj.get(name)
        .and_then(Value::as_f64)
        .ok_or_else(|| format!("{name} must be a number"))
}

fn as_i64(obj: &Map<String, Value>, name: &str) -> Result<i64, String> {
    obj.get(name)
        .and_then(Value::as_i64)
        .ok_or_else(|| format!("{name} must be an integer"))
}

fn optional_i64(obj: &Map<String, Value>, name: &str) -> Result<Option<i64>, String> {
    match obj.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_i64()
            .map(Some)
            .ok_or_else(|| format!("{name} must be an integer or null")),
    }
}

/// `ExitParams::from_pairs` over the exporter's `exit_params` mapping. A JSON number with no
/// fractional part becomes an `Int`, which is what the period and bar-count parameters want.
fn params_from_json(value: &Value) -> Result<ExitParams, String> {
    let map = value
        .as_object()
        .ok_or_else(|| "exit_params must be an object".to_string())?;
    let mut pairs = Vec::with_capacity(map.len());
    for (name, raw) in map {
        let param = if let Some(flag) = raw.as_bool() {
            ParamValue::Bool(flag)
        } else if let Some(integer) = raw.as_i64() {
            ParamValue::Int(integer)
        } else if let Some(float) = raw.as_f64() {
            if float.is_finite() && float.fract().to_bits() == 0.0_f64.to_bits() {
                ParamValue::Int(float as i64)
            } else {
                ParamValue::Float(float)
            }
        } else {
            return Err(format!("unsupported exit parameter {name}: {raw}"));
        };
        pairs.push((name.as_str(), param));
    }
    ExitParams::from_pairs(pairs).map_err(|e| format!("exit_params: {e:?}"))
}

/// The backend's `PositionSizingConfig.model_dump` mapped onto [`Sizing`].
///
/// `sizing_point_value` is the point value the evaluator handed `build_position_sizer`, which
/// only the inverse-volatility model reads.
fn sizing_from_json(value: &Value, sizing_point_value: f64) -> Result<Sizing, String> {
    let obj = value
        .as_object()
        .ok_or_else(|| "sizing must be an object".to_string())?;
    let kind = obj
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| "sizing.type must be a string".to_string())?;
    let scale_by_strength = obj
        .get("scale_by_signal_strength")
        .and_then(Value::as_bool)
        .ok_or_else(|| "sizing.scale_by_signal_strength must be a bool".to_string())?;

    match kind {
        "fixed_quantity" => Ok(Sizing::FixedQuantity {
            quantity: as_f64(obj, "quantity")?,
            scale_by_strength,
        }),
        "fixed_safety_margin" => Ok(Sizing::FixedSafetyMargin {
            margin_per_contract: as_f64(obj, "safety_margin_per_contract")?,
            min_contracts: as_i64(obj, "min_contracts")?,
            max_contracts: optional_i64(obj, "max_contracts")?,
            scale_by_strength,
        }),
        "inverse_volatility" => Ok(Sizing::InverseVolatility {
            target_volatility_pct: as_f64(obj, "target_volatility_pct")?,
            point_value: sizing_point_value,
            min_contracts: as_i64(obj, "min_contracts")?,
            max_contracts: optional_i64(obj, "max_contracts")?,
            scale_by_strength,
        }),
        other => Err(format!("unknown sizing type {other}")),
    }
}

/// The scenario's single open trade. `entry_bar` is `None` when the entry time is not a bar
/// of the series, which is how `_timestamp_to_bar.get` comes up empty for d07.
#[derive(Clone, Copy, Debug)]
struct OpenTrade {
    side: Side,
    entry_price: f64,
    entry_bar: Option<usize>,
}

fn open_trade_from_json(value: Option<&Value>) -> Result<Option<OpenTrade>, String> {
    let Some(raw) = value.filter(|v| !v.is_null()) else {
        return Ok(None);
    };
    let obj = raw
        .as_object()
        .ok_or_else(|| "open_trade must be an object or null".to_string())?;
    let side = match as_i64(obj, "side")? {
        1 => Side::Long,
        -1 => Side::Short,
        other => return Err(format!("open_trade.side {other} is not 1 or -1")),
    };
    let entry_bar = optional_i64(obj, "entry_bar")?
        .map(|bar| usize::try_from(bar).map_err(|_| format!("negative entry_bar {bar}")))
        .transpose()?;
    Ok(Some(OpenTrade {
        side,
        entry_price: as_f64(obj, "entry_price")?,
        entry_bar,
    }))
}

// ---------------------------------------------------------------------------
// One case, decoded
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
struct Case {
    close: Vec<f64>,
    high: Option<Vec<f64>>,
    low: Option<Vec<f64>>,
    entry: Vec<i8>,
    exit_long: Vec<bool>,
    exit_short: Vec<bool>,
    strength: Vec<f64>,
    bar_index: Option<Vec<i64>>,
    volatility: Option<Vec<f64>>,
    /// Exit-rule indicator columns, by their fixture name.
    named: BTreeMap<String, Vec<f64>>,
    initial_capital: f64,
    sizing: Sizing,
    holding_period_bars: Option<i64>,
    exit_params: ExitParams,
    open_trade: Option<OpenTrade>,
}

impl Case {
    fn from_json(case: &Value) -> Result<Self, String> {
        let inputs = case
            .get("inputs")
            .and_then(Value::as_object)
            .ok_or_else(|| "missing inputs".to_string())?;
        let config = case
            .get("config")
            .and_then(Value::as_object)
            .ok_or_else(|| "missing config".to_string())?;

        let named = inputs
            .iter()
            .filter(|(name, _)| name.starts_with("atr_") || name.starts_with("donchian_"))
            .map(|(name, value)| float_column(value, name).map(|column| (name.clone(), column)))
            .collect::<Result<BTreeMap<_, _>, _>>()?;

        Ok(Self {
            close: float_column(required(inputs, "close")?, "close")?,
            high: optional_floats(inputs, "high")?,
            low: optional_floats(inputs, "low")?,
            entry: entry_column(required(inputs, ENTRY_COLUMN)?, ENTRY_COLUMN)?,
            exit_long: bool_column(required(inputs, EXIT_LONG_COLUMN)?, EXIT_LONG_COLUMN)?,
            exit_short: bool_column(required(inputs, EXIT_SHORT_COLUMN)?, EXIT_SHORT_COLUMN)?,
            strength: float_column(required(inputs, STRENGTH_COLUMN)?, STRENGTH_COLUMN)?,
            bar_index: inputs
                .get(BAR_INDEX_COLUMN)
                .map(|value| int_column(value, BAR_INDEX_COLUMN))
                .transpose()?,
            volatility: optional_floats(inputs, "volatility")?,
            named,
            initial_capital: as_f64(config, "initial_capital")?,
            sizing: sizing_from_json(
                config.get("sizing").ok_or("missing sizing")?,
                as_f64(config, "sizing_point_value")?,
            )?,
            holding_period_bars: optional_i64(config, "holding_period_bars")?,
            exit_params: params_from_json(config.get("exit_params").ok_or("missing exit_params")?)?,
            open_trade: open_trade_from_json(config.get("open_trade"))?,
        })
    }

    /// One [`DecisionStep`] walked across every bar with the scenario's open trade, in the
    /// layout `encode_decisions` wrote.
    fn replay(&self) -> Result<Trace, String> {
        let rules = ExitRuleSet::new(self.exit_params.clone());
        let lookup = |name: &str| self.named.get(name).map(Vec::as_slice);
        let exit_inputs = ExitInputs::from_slices(
            &self.close,
            self.high.as_deref(),
            self.low.as_deref(),
            &lookup,
            &rules,
        )
        .map_err(|e| format!("{e:?}"))?;
        let signals = SignalColumns {
            entry: &self.entry,
            exit_long: &self.exit_long,
            exit_short: &self.exit_short,
            strength: &self.strength,
            bar_index: self.bar_index.as_deref(),
        };

        // The reference re-evaluates the same open trade on every bar: it records what each
        // closed bar would queue and never fills anything, so the trade never closes.
        let open: Vec<TradeView> = self
            .open_trade
            .iter()
            .map(|trade| TradeView {
                key: PositionKey(0),
                side: trade.side,
                entry_price: trade.entry_price,
                entry_bar: trade.entry_bar,
            })
            .collect();

        let mut step = DecisionStep::new(self.exit_params.clone());
        let mut trace = Trace::with_capacity(self.close.len());
        for bar in 0..self.close.len() {
            let decision =
                step.decide(bar, &open, &signals, &exit_inputs, self.holding_period_bars);
            trace.push(bar, &decision, self);
        }
        Ok(trace)
    }

    /// `StrategyEvaluator._evaluate_row`: a queued close makes the bar's action CLOSE, and
    /// CLOSE sizes nothing. Only an entry with nothing closing reaches the sizer, at the
    /// bar's close price and the initial capital. A sizer that declines records NaN, which
    /// is how the exporter encoded `requested_quantity = None`.
    fn requested_quantity(&self, bar: usize, decision: &Decision) -> f64 {
        if !decision.exits.is_empty() {
            return f64::NAN;
        }
        let Some(entry) = decision.entry else {
            return f64::NAN;
        };
        self.sizing
            .size(
                entry.strength,
                self.close[bar],
                self.initial_capital,
                self.volatility.as_ref().map(|column| column[bar]),
            )
            .unwrap_or(f64::NAN)
    }
}

// ---------------------------------------------------------------------------
// The replayed trace
// ---------------------------------------------------------------------------

/// `DecisionTrace`'s layout plus the evaluator's requested quantity, one row per bar.
///
/// Bar `i`'s queued closes are `exit_reason[exit_offsets[i]..exit_offsets[i + 1]]`, so a bar
/// that queues nothing contributes an empty range rather than a sentinel row.
#[derive(Clone, Debug)]
struct Trace {
    exit_offsets: Vec<i64>,
    exit_reason: Vec<i64>,
    entry: Vec<i64>,
    entry_strength: Vec<f64>,
    requested_quantity: Vec<f64>,
}

impl Trace {
    fn with_capacity(bars: usize) -> Self {
        let mut exit_offsets = Vec::with_capacity(bars + 1);
        exit_offsets.push(0);
        Self {
            exit_offsets,
            exit_reason: Vec::new(),
            entry: Vec::with_capacity(bars),
            entry_strength: Vec::with_capacity(bars),
            requested_quantity: Vec::with_capacity(bars),
        }
    }

    fn push(&mut self, bar: usize, decision: &Decision, case: &Case) {
        for exit in &decision.exits {
            self.exit_reason.push(
                exit.rule
                    .map_or(ExitReason::Signal, ExitReason::Rule)
                    .code(),
            );
        }
        self.exit_offsets.push(self.exit_reason.len() as i64);
        let (entry, strength) = match decision.entry {
            Some(entry) => (
                match entry.side {
                    Side::Long => 1,
                    Side::Short => -1,
                },
                entry.strength,
            ),
            None => (0, 0.0),
        };
        self.entry.push(entry);
        self.entry_strength.push(strength);
        self.requested_quantity
            .push(case.requested_quantity(bar, decision));
    }

    fn ints(&self, name: &str) -> Vec<i64> {
        match name {
            "entry" => self.entry.clone(),
            "exit_offsets" => self.exit_offsets.clone(),
            "exit_reason" => self.exit_reason.clone(),
            other => unreachable!("{other} is not a trace int column"),
        }
    }

    fn floats(&self, name: &str) -> Vec<f64> {
        match name {
            "entry_strength" => self.entry_strength.clone(),
            "requested_quantity" => self.requested_quantity.clone(),
            other => unreachable!("{other} is not a trace float column"),
        }
    }
}

// ---------------------------------------------------------------------------
// Comparison
// ---------------------------------------------------------------------------

fn compare_i64(expected: &[i64], actual: &[i64]) -> Result<(), (usize, MismatchKind)> {
    if expected.len() != actual.len() {
        return Err((
            0,
            MismatchKind::Length {
                expected: expected.len(),
                actual: actual.len(),
            },
        ));
    }
    for (index, (&want, &got)) in expected.iter().zip(actual).enumerate() {
        if want != got {
            return Err((index, MismatchKind::Int64));
        }
    }
    Ok(())
}

fn case_id(file: &FixtureFile) -> String {
    file.cases
        .first()
        .and_then(|case| case.get("case_id"))
        .and_then(Value::as_str)
        .unwrap_or("*")
        .to_string()
}

fn golden(
    file: &FixtureFile,
    column: &str,
    index: usize,
    expected: String,
    actual: String,
    kind: MismatchKind,
) -> Box<GateFailure> {
    Box::new(GateFailure::Golden {
        function_id: file.fixture_id.clone(),
        case_id: case_id(file),
        mismatch: Mismatch {
            output: column.to_string(),
            index,
            expected,
            actual,
            kind,
        },
    })
}

fn unexpected(file: &FixtureFile, message: String) -> Box<GateFailure> {
    Box::new(GateFailure::UnexpectedError {
        function_id: file.fixture_id.clone(),
        case_id: case_id(file),
        message,
    })
}

fn compare_trace(file: &FixtureFile, trace: &Trace) -> Result<(), Box<GateFailure>> {
    let expected = file
        .cases
        .first()
        .and_then(|case| case.get("expected"))
        .and_then(Value::as_object)
        .ok_or_else(|| unexpected(file, "missing expected".to_string()))?;

    for &name in TRACE_INTS {
        let raw = expected
            .get(name)
            .ok_or_else(|| unexpected(file, format!("missing expected column {name}")))?;
        let want = int_column(raw, name).map_err(|e| unexpected(file, e))?;
        let got = trace.ints(name);
        if let Err((index, kind)) = compare_i64(&want, &got) {
            let show = |values: &[i64]| {
                values
                    .get(index)
                    .map_or_else(|| "<none>".to_string(), i64::to_string)
            };
            return Err(golden(file, name, index, show(&want), show(&got), kind));
        }
    }

    for &name in TRACE_FLOATS {
        let raw = expected
            .get(name)
            .ok_or_else(|| unexpected(file, format!("missing expected column {name}")))?;
        let want = float_column(raw, name).map_err(|e| unexpected(file, e))?;
        let got = trace.floats(name);
        if let Err((index, kind)) = compare_f64(&file.policy, &want, &got) {
            let show = |values: &[f64]| {
                values
                    .get(index)
                    .map_or_else(|| "<none>".to_string(), |value| format!("{value:?}"))
            };
            return Err(golden(file, name, index, show(&want), show(&got), kind));
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn decision_step_reference_gate() {
    let files = load_family(&fixtures_root(), "decision_step").expect("load_family");
    let pending = parse_pending("").expect("parse_pending");
    let mut failures = check_accounting(&files, BOUND_SCENARIOS, &pending, BACKEND_REV.trim());

    for file in &files {
        let case = match file
            .cases
            .first()
            .ok_or_else(|| "no cases".to_string())
            .and_then(Case::from_json)
        {
            Ok(case) => case,
            Err(message) => {
                failures.push(*unexpected(file, message));
                continue;
            }
        };
        match case.replay() {
            Ok(trace) => {
                if let Err(failure) = compare_trace(file, &trace) {
                    failures.push(*failure);
                }
            }
            Err(message) => failures.push(*unexpected(file, message)),
        }
    }

    let report = GateReport {
        bound: BOUND_SCENARIOS.iter().map(|id| (*id).to_string()).collect(),
        pending: Vec::new(),
        failures,
    };
    println!("{}", report.summary());
    report.assert_passed();
}
