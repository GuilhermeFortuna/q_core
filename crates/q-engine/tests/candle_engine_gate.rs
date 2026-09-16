#![forbid(unsafe_code)]

//! `candle_engine` reference gate: the ledger `run_candle` writes for each scenario must
//! match the one `BacktestEngine._run_single_chunk` wrote, column by column and bit by bit.

use q_engine::{
    run_candle, CandleConfig, CandleInputs, CandleRun, Costs, DayTradeWindow, ExitParams,
    ParamValue, SignalColumns, Sizing, TradeLedger,
};
use q_parity::compare::{compare_f64, Mismatch, MismatchKind};
use q_parity::determinism::{check_double_run_with, BitEq};
use q_parity::fixture::FixtureFile;
use q_parity::fixture::{fnv1a64_float64, fnv1a64_int64, load_family, Column, ColumnData};
use q_parity::gate::{check_accounting, parse_pending, GateFailure, GateReport};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const BACKEND_REV: &str = include_str!("../../../BACKEND_REV");

const BOUND_SCENARIOS: &[&str] = &[
    "k01_strategy_exits",
    "k02_stop_target_rules",
    "k03_atr_trailing_rules",
    "k04_long_short_both_open",
    "k05_rule_and_strategy_same_bar",
    "k06_pyramid_cap_trim",
    "k07_safety_margin_compounding",
    "k08_inverse_volatility",
    "k09_strength_floor_to_zero",
    "k10_costs",
    "k11_holding_period",
    "k12_day_trade_sequential",
    "k13_day_trade_chunk_force_close",
    "k14_trade_start_warmup",
    "k15_close_only_series",
    "k16_repeated_times",
    "k17_all_rules_psar_ratchet_breakeven",
];

/// Ledger columns the exporter writes as int64.
const LEDGER_INTS: &[&str] = &["entry_bar", "exit_bar", "side", "exit_reason"];
/// Ledger columns the exporter writes as float64.
const LEDGER_FLOATS: &[&str] = &["commission", "entry_price", "exit_price", "pnl", "quantity"];

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

/// The exporter stores the two exit columns as 0/1 int64; anything else is a contract break.
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
/// `sizing_point_value` is the point value the engine handed `build_position_sizer`, which
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

fn costs_from_json(value: Option<&Value>) -> Result<Option<Costs>, String> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(raw) => {
            let obj = raw
                .as_object()
                .ok_or_else(|| "costs must be an object or null".to_string())?;
            Ok(Some(Costs {
                per_contract: as_f64(obj, "per_contract")?,
                bps: as_f64(obj, "bps")?,
            }))
        }
    }
}

fn day_trade_from_json(value: Option<&Value>) -> Result<Option<DayTradeWindow>, String> {
    let Some(raw) = value.filter(|v| !v.is_null()) else {
        return Ok(None);
    };
    let times = raw
        .as_array()
        .ok_or_else(|| "day_trade_us must be an array or null".to_string())?;
    if times.len() != 3 {
        return Err(format!(
            "day_trade_us must hold three times, got {}",
            times.len()
        ));
    }
    let at = |index: usize| -> Result<i64, String> {
        times[index]
            .as_i64()
            .ok_or_else(|| format!("day_trade_us[{index}] must be an integer"))
    };
    Ok(Some(DayTradeWindow {
        entry_start_us: at(0)?,
        entry_end_us: at(1)?,
        force_close_us: at(2)?,
    }))
}

// ---------------------------------------------------------------------------
// One case, decoded
// ---------------------------------------------------------------------------

/// Every column and parameter of one scenario, owned so a prefix can be taken.
#[derive(Clone, Debug)]
struct Case {
    time_us: Vec<i64>,
    open: Option<Vec<f64>>,
    high: Option<Vec<f64>>,
    low: Option<Vec<f64>>,
    close: Option<Vec<f64>>,
    entry: Vec<i8>,
    exit_long: Vec<bool>,
    exit_short: Vec<bool>,
    strength: Vec<f64>,
    bar_index: Option<Vec<i64>>,
    volatility: Option<Vec<f64>>,
    tradable: Option<Vec<bool>>,
    /// Exit-rule indicator columns, by their fixture name.
    named: BTreeMap<String, Vec<f64>>,
    config: CandleConfig,
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

        let sizing_point_value = as_f64(config, "sizing_point_value")?;
        let candle_config = CandleConfig {
            initial_capital: as_f64(config, "initial_capital")?,
            point_value: as_f64(config, "point_value")?,
            costs: costs_from_json(config.get("costs"))?,
            sizing: sizing_from_json(
                config.get("sizing").ok_or("missing sizing")?,
                sizing_point_value,
            )?,
            holding_period_bars: optional_i64(config, "holding_period_bars")?,
            exit_params: params_from_json(config.get("exit_params").ok_or("missing exit_params")?)?,
            day_trade: day_trade_from_json(config.get("day_trade_us"))?,
            force_close_at_end: config
                .get("force_close_at_end")
                .and_then(Value::as_bool)
                .ok_or_else(|| "force_close_at_end must be a bool".to_string())?,
        };

        let named = inputs
            .iter()
            .filter(|(name, _)| name.starts_with("atr_") || name.starts_with("donchian_"))
            .map(|(name, value)| float_column(value, name).map(|column| (name.clone(), column)))
            .collect::<Result<BTreeMap<_, _>, _>>()?;

        let tradable = inputs
            .get("tradable")
            .map(|value| bool_column(value, "tradable"))
            .transpose()?;
        // The exporter derives `tradable` from `trade_start_bar`; reading only the column
        // would let the config field drift unnoticed.
        check_trade_start(
            optional_i64(config, "trade_start_bar")?,
            tradable.as_deref(),
        )?;

        Ok(Self {
            time_us: int_column(required(inputs, "time_us")?, "time_us")?,
            open: optional_floats(inputs, "open")?,
            high: optional_floats(inputs, "high")?,
            low: optional_floats(inputs, "low")?,
            close: optional_floats(inputs, "close")?,
            entry: entry_column(required(inputs, ENTRY_COLUMN)?, ENTRY_COLUMN)?,
            exit_long: bool_column(required(inputs, EXIT_LONG_COLUMN)?, EXIT_LONG_COLUMN)?,
            exit_short: bool_column(required(inputs, EXIT_SHORT_COLUMN)?, EXIT_SHORT_COLUMN)?,
            strength: float_column(required(inputs, STRENGTH_COLUMN)?, STRENGTH_COLUMN)?,
            bar_index: inputs
                .get(BAR_INDEX_COLUMN)
                .map(|value| int_column(value, BAR_INDEX_COLUMN))
                .transpose()?,
            volatility: optional_floats(inputs, "volatility")?,
            tradable,
            named,
            config: candle_config,
        })
    }

    fn len(&self) -> usize {
        self.time_us.len()
    }

    fn run(&self) -> Result<CandleRun, String> {
        let lookup = |name: &str| self.named.get(name).map(Vec::as_slice);
        let inputs = CandleInputs {
            time_us: &self.time_us,
            open: self.open.as_deref(),
            high: self.high.as_deref(),
            low: self.low.as_deref(),
            close: self.close.as_deref(),
            signals: SignalColumns {
                entry: &self.entry,
                exit_long: &self.exit_long,
                exit_short: &self.exit_short,
                strength: &self.strength,
                bar_index: self.bar_index.as_deref(),
            },
            volatility: self.volatility.as_deref(),
            tradable: self.tradable.as_deref(),
            columns: &lookup,
        };
        run_candle(&inputs, &self.config).map_err(|e| format!("{e:?}"))
    }

    /// The same scenario over its first `len` bars.
    fn prefix(&self, len: usize) -> Self {
        let len = len.min(self.len());
        let cut_floats =
            |column: &Option<Vec<f64>>| column.as_ref().map(|values| values[..len].to_vec());
        Self {
            time_us: self.time_us[..len].to_vec(),
            open: cut_floats(&self.open),
            high: cut_floats(&self.high),
            low: cut_floats(&self.low),
            close: cut_floats(&self.close),
            entry: self.entry[..len].to_vec(),
            exit_long: self.exit_long[..len].to_vec(),
            exit_short: self.exit_short[..len].to_vec(),
            strength: self.strength[..len].to_vec(),
            bar_index: self.bar_index.as_ref().map(|values| values[..len].to_vec()),
            volatility: cut_floats(&self.volatility),
            tradable: self.tradable.as_ref().map(|values| values[..len].to_vec()),
            named: self
                .named
                .iter()
                .map(|(name, values)| (name.clone(), values[..len].to_vec()))
                .collect(),
            config: self.config.clone(),
        }
    }
}

fn check_trade_start(
    trade_start_bar: Option<i64>,
    tradable: Option<&[bool]>,
) -> Result<(), String> {
    match (trade_start_bar, tradable) {
        (None, None) => Ok(()),
        (None, Some(_)) => Err("tradable column without a trade_start_bar".to_string()),
        (Some(bar), None) => Err(format!("trade_start_bar {bar} without a tradable column")),
        (Some(bar), Some(flags)) => {
            let start =
                usize::try_from(bar).map_err(|_| format!("negative trade_start_bar {bar}"))?;
            for (index, &flag) in flags.iter().enumerate() {
                if flag != (index >= start) {
                    return Err(format!(
                        "tradable[{index}] is {flag} but trade_start_bar is {start}"
                    ));
                }
            }
            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// Ledger comparison
// ---------------------------------------------------------------------------

fn actual_ints(ledger: &TradeLedger, name: &str) -> Vec<i64> {
    match name {
        "entry_bar" => ledger.entry_bar.clone(),
        "exit_bar" => ledger.exit_bar.clone(),
        // The fixture stores the side as int64; the ledger holds it as i8.
        "side" => ledger.side.iter().map(|&side| i64::from(side)).collect(),
        "exit_reason" => ledger.exit_reason.clone(),
        other => unreachable!("{other} is not a ledger int column"),
    }
}

fn actual_floats(ledger: &TradeLedger, name: &str) -> Vec<f64> {
    match name {
        "commission" => ledger.commission.clone(),
        "entry_price" => ledger.entry_price.clone(),
        "exit_price" => ledger.exit_price.clone(),
        "pnl" => ledger.pnl.clone(),
        "quantity" => ledger.quantity.clone(),
        other => unreachable!("{other} is not a ledger float column"),
    }
}

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

fn case_id(file: &FixtureFile) -> String {
    file.cases
        .first()
        .and_then(|case| case.get("case_id"))
        .and_then(Value::as_str)
        .unwrap_or("*")
        .to_string()
}

/// Every ledger column under the fixture's own policy, plus the open-trade sentinels.
fn compare_ledger(file: &FixtureFile, ledger: &TradeLedger) -> Result<(), Box<GateFailure>> {
    let expected = file
        .cases
        .first()
        .and_then(|case| case.get("expected"))
        .and_then(Value::as_object)
        .ok_or_else(|| unexpected(file, "missing expected".to_string()))?;

    for &name in LEDGER_INTS {
        let raw = expected
            .get(name)
            .ok_or_else(|| unexpected(file, format!("missing expected column {name}")))?;
        let want = int_column(raw, name).map_err(|e| unexpected(file, e))?;
        let got = actual_ints(ledger, name);
        if let Err((index, kind)) = compare_i64(&want, &got) {
            let (expected_text, actual_text) = describe_i64(&want, &got, index);
            return Err(golden(file, name, index, expected_text, actual_text, kind));
        }
    }

    for &name in LEDGER_FLOATS {
        let raw = expected
            .get(name)
            .ok_or_else(|| unexpected(file, format!("missing expected column {name}")))?;
        let want = float_column(raw, name).map_err(|e| unexpected(file, e))?;
        let got = actual_floats(ledger, name);
        if let Err((index, kind)) = compare_f64(&file.policy, &want, &got) {
            let (expected_text, actual_text) = describe_f64(&want, &got, index);
            return Err(golden(file, name, index, expected_text, actual_text, kind));
        }
    }

    check_open_trades(file, ledger)
}

/// `TradeLedger` leaves an open trade at `exit_bar = -1`, `exit_reason = -1` and NaN exit
/// price and pnl. The fixture encodes the same sentinels, so a disagreement would already
/// show up above; this pins the invariant itself against a kernel that stopped writing it.
fn check_open_trades(file: &FixtureFile, ledger: &TradeLedger) -> Result<(), Box<GateFailure>> {
    for (index, &exit_bar) in ledger.exit_bar.iter().enumerate() {
        let open = exit_bar < 0;
        let sentinels = [
            ("exit_bar", exit_bar == -1),
            ("exit_reason", ledger.exit_reason[index] == -1),
            ("exit_price", ledger.exit_price[index].is_nan()),
            ("pnl", ledger.pnl[index].is_nan()),
        ];
        for (column, is_sentinel) in sentinels {
            if open != is_sentinel {
                return Err(unexpected(
                    file,
                    format!(
                        "trade {index}: exit_bar {exit_bar} disagrees with {column}'s open-trade sentinel"
                    ),
                ));
            }
        }
    }
    Ok(())
}

fn describe_i64(expected: &[i64], actual: &[i64], index: usize) -> (String, String) {
    (
        expected
            .get(index)
            .map_or_else(|| "<none>".to_string(), i64::to_string),
        actual
            .get(index)
            .map_or_else(|| "<none>".to_string(), i64::to_string),
    )
}

fn describe_f64(expected: &[f64], actual: &[f64], index: usize) -> (String, String) {
    let show = |values: &[f64]| {
        values
            .get(index)
            .map_or_else(|| "<none>".to_string(), |value| format!("{value:?}"))
    };
    (show(expected), show(actual))
}

// ---------------------------------------------------------------------------
// Determinism
// ---------------------------------------------------------------------------

/// A whole run as comparable columns, so double-run equality covers the trace too.
struct Snapshot {
    ints: BTreeMap<&'static str, Vec<i64>>,
    floats: BTreeMap<&'static str, Vec<f64>>,
}

impl Snapshot {
    fn of(run: &CandleRun) -> Self {
        let mut ints = BTreeMap::new();
        for &name in LEDGER_INTS {
            ints.insert(name, actual_ints(&run.trades, name));
        }
        ints.insert("trace_exit_offsets", run.trace.exit_offsets.clone());
        ints.insert("trace_exit_reason", run.trace.exit_reason.clone());
        ints.insert(
            "trace_entry",
            run.trace
                .entry
                .iter()
                .map(|&side| i64::from(side))
                .collect(),
        );

        let mut floats = BTreeMap::new();
        for &name in LEDGER_FLOATS {
            floats.insert(name, actual_floats(&run.trades, name));
        }
        floats.insert("trace_entry_strength", run.trace.entry_strength.clone());

        Self { ints, floats }
    }
}

impl BitEq for Snapshot {
    fn bit_eq(&self, other: &Self) -> Result<(), String> {
        for (name, values) in &self.ints {
            let theirs = other
                .ints
                .get(name)
                .ok_or_else(|| format!("missing column {name}"))?;
            values
                .bit_eq(theirs)
                .map_err(|e| format!("column {name}: {e}"))?;
        }
        for (name, values) in &self.floats {
            let theirs = other
                .floats
                .get(name)
                .ok_or_else(|| format!("missing column {name}"))?;
            values
                .bit_eq(theirs)
                .map_err(|e| format!("column {name}: {e}"))?;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

fn load_cases() -> Vec<(FixtureFile, Case)> {
    let files = load_family(&fixtures_root(), "candle_engine").expect("load_family");
    files
        .into_iter()
        .map(|file| {
            let case = Case::from_json(file.cases.first().expect("one case per fixture"))
                .unwrap_or_else(|e| panic!("{}: {e}", file.fixture_id));
            (file, case)
        })
        .collect()
}

#[test]
fn candle_engine_reference_gate() {
    let files = load_family(&fixtures_root(), "candle_engine").expect("load_family");
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
        match case.run() {
            Ok(run) => {
                if let Err(failure) = compare_ledger(file, &run.trades) {
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

#[test]
fn candle_engine_double_run() {
    for (file, case) in load_cases() {
        check_double_run_with(|| Snapshot::of(&case.run().expect("run")))
            .unwrap_or_else(|e| panic!("{} nondeterministic: {e}", file.fixture_id));
    }
}

/// The decision trace over the first `k` bars must equal the full run's first `k` entries.
///
/// A day-trade window is the one case where truncation legitimately changes a decision. The
/// backend's `is_last_bar_of_day` is `i == len(chunk) - 1 or chunk.index[i + 1].date() !=
/// timestamp.date()`, so a chunk's final bar counts as the last of its day and queues
/// nothing. Shortening the series moves that bar, and the kernel reproduces it faithfully.
/// Those scenarios are therefore compared up to, but not including, the prefix's last bar;
/// every other scenario is compared over the whole prefix.
#[test]
fn candle_engine_prefix_causality() {
    for (file, case) in load_cases() {
        let n = case.len();
        let full = case.run().expect("full run");
        for length in [1, n / 3, (2 * n) / 3, n] {
            if length == 0 {
                continue;
            }
            let prefix = case.prefix(length).run().expect("prefix run");
            let bars = if case.config.day_trade.is_some() {
                length - 1
            } else {
                length
            };
            assert_prefix(&file.fixture_id, &full, &prefix, length, bars);
        }
    }
}

fn assert_prefix(id: &str, full: &CandleRun, prefix: &CandleRun, length: usize, bars: usize) {
    assert_eq!(
        prefix.trace.entry.len(),
        length,
        "{id}: prefix of {length} bars traced {} entries",
        prefix.trace.entry.len()
    );
    for bar in 0..bars {
        assert_eq!(
            full.trace.entry[bar], prefix.trace.entry[bar],
            "{id}: entry differs at bar {bar} for prefix {length}"
        );
        assert_eq!(
            full.trace.entry_strength[bar].to_bits(),
            prefix.trace.entry_strength[bar].to_bits(),
            "{id}: entry_strength differs at bar {bar} for prefix {length}"
        );
        assert_eq!(
            full.trace.exit_offsets[bar + 1] - full.trace.exit_offsets[bar],
            prefix.trace.exit_offsets[bar + 1] - prefix.trace.exit_offsets[bar],
            "{id}: queued exit count differs at bar {bar} for prefix {length}"
        );
    }
    let queued = usize::try_from(full.trace.exit_offsets[bars]).expect("non-negative offset");
    assert_eq!(
        full.trace.exit_reason[..queued],
        prefix.trace.exit_reason[..queued],
        "{id}: queued exit reasons differ over the first {bars} bars of prefix {length}"
    );
}

// ---------------------------------------------------------------------------
// Negative controls
// ---------------------------------------------------------------------------

fn clone_fixture(file: &FixtureFile) -> FixtureFile {
    FixtureFile {
        path: file.path.clone(),
        family: file.family.clone(),
        fixture_id: file.fixture_id.clone(),
        policy: file.policy.clone(),
        provenance: file.provenance.clone(),
        cases: file.cases.clone(),
    }
}

fn reseal_floats(case: &mut Value, column: &str) {
    let values: Vec<f64> = case["expected"][column]["values"]
        .as_array()
        .expect("values array")
        .iter()
        .map(|value| value.as_f64().unwrap_or(f64::NAN))
        .collect();
    let checksum = fnv1a64_float64(&values);
    case["expected"][column]["bits_fnv1a64"] = Value::String(format!("0x{checksum:016x}"));
}

fn reseal_ints(case: &mut Value, column: &str) {
    let values: Vec<i64> = case["expected"][column]["values"]
        .as_array()
        .expect("values array")
        .iter()
        .map(|value| value.as_i64().expect("int value"))
        .collect();
    let checksum = fnv1a64_int64(&values);
    case["expected"][column]["bits_fnv1a64"] = Value::String(format!("0x{checksum:016x}"));
}

/// Run one mutated fixture and return the comparison's verdict.
fn verdict(file: &FixtureFile, case: Value) -> Result<(), Box<GateFailure>> {
    let mut mutated = clone_fixture(file);
    mutated.cases[0] = case;
    let parsed = Case::from_json(&mutated.cases[0]).expect("inputs are untouched");
    let run = parsed.run().expect("run");
    compare_ledger(&mutated, &run.trades)
}

#[test]
fn candle_engine_negative_controls() {
    let root = fixtures_root();
    let files = load_family(&root, "candle_engine").expect("load_family");
    let file = files
        .iter()
        .find(|file| file.fixture_id == "k01_strategy_exits")
        .expect("k01_strategy_exits");

    // 1) One ULP on the first entry price.
    {
        let mut case = file.cases[0].clone();
        let values = case["expected"]["entry_price"]["values"]
            .as_array_mut()
            .expect("entry_price");
        let bits = values[0].as_f64().expect("finite price").to_bits() + 1;
        values[0] = serde_json::Number::from_f64(f64::from_bits(bits))
            .map(Value::Number)
            .expect("finite perturbation");
        reseal_floats(&mut case, "entry_price");
        match *verdict(file, case).expect_err("one ULP must fail") {
            GateFailure::Golden { mismatch, .. } if mismatch.kind == MismatchKind::Bits => {}
            other => panic!("expected a Bits mismatch, got {other:?}"),
        }
    }

    // 2) Move the first close one bar later.
    {
        let mut case = file.cases[0].clone();
        let values = case["expected"]["exit_bar"]["values"]
            .as_array_mut()
            .expect("exit_bar");
        let index = values
            .iter()
            .position(|value| value.as_i64().is_some_and(|bar| bar >= 0))
            .expect("a closed trade");
        let moved = values[index].as_i64().expect("int bar") + 1;
        values[index] = serde_json::json!(moved);
        reseal_ints(&mut case, "exit_bar");
        match *verdict(file, case).expect_err("a shifted exit bar must fail") {
            GateFailure::Golden { mismatch, .. } => {
                assert_eq!(mismatch.output, "exit_bar");
                assert_eq!(mismatch.kind, MismatchKind::Int64);
            }
            other => panic!("expected a Golden mismatch, got {other:?}"),
        }
    }

    // 3) Relabel a scripted close as an end-of-day one.
    {
        let mut case = file.cases[0].clone();
        let values = case["expected"]["exit_reason"]["values"]
            .as_array_mut()
            .expect("exit_reason");
        let index = values
            .iter()
            .position(|value| value.as_i64() == Some(11))
            .expect("a SIGNAL close");
        values[index] = serde_json::json!(12);
        reseal_ints(&mut case, "exit_reason");
        match *verdict(file, case).expect_err("a relabelled reason must fail") {
            GateFailure::Golden { mismatch, .. } => {
                assert_eq!(mismatch.output, "exit_reason");
            }
            other => panic!("expected a Golden mismatch, got {other:?}"),
        }
    }

    // 4) Edit a value and leave its checksum behind.
    {
        let mut case = file.cases[0].clone();
        *case
            .pointer_mut("/expected/pnl/values/0")
            .expect("first pnl") = serde_json::json!(42.0);
        match *verdict(file, case).expect_err("a stale checksum must fail") {
            GateFailure::UnexpectedError { message, .. } => {
                assert!(
                    message.contains("checksum mismatch"),
                    "expected a checksum failure, got {message}"
                );
            }
            other => panic!("expected an UnexpectedError, got {other:?}"),
        }
    }

    // 5) A fixture file no binding accounts for.
    {
        let staged = stage_family(&root);
        std::fs::write(
            staged.join("candle_engine/zz_extra.json"),
            r#"{"format":"q-core-reference-fixture/1","family":"candle_engine","fixture_id":"zz_extra","policy":{"kind":"exact"},"provenance":{"backend_rev":"x"},"cases":[]}"#,
        )
        .expect("write the extra fixture");
        let staged_files = load_family(&staged, "candle_engine").expect("load_family");
        let pending = parse_pending("").expect("parse_pending");
        let failures =
            check_accounting(&staged_files, BOUND_SCENARIOS, &pending, BACKEND_REV.trim());
        assert!(
            failures
                .iter()
                .any(|failure| matches!(failure, GateFailure::Unaccounted { .. })),
            "expected an Unaccounted failure, got {failures:?}"
        );
        let _ = std::fs::remove_dir_all(&staged);
    }
}

fn stage_family(root: &Path) -> PathBuf {
    let staged = std::env::temp_dir().join(format!("q027-candle-engine-{}", std::process::id()));
    let family = staged.join("candle_engine");
    let _ = std::fs::remove_dir_all(&staged);
    std::fs::create_dir_all(&family).expect("create the staging directory");
    for entry in std::fs::read_dir(root.join("candle_engine")).expect("read the fixture directory")
    {
        let entry = entry.expect("directory entry");
        std::fs::copy(entry.path(), family.join(entry.file_name())).expect("copy the fixture");
    }
    staged
}
