#![forbid(unsafe_code)]

use q_engine::{
    resolve_bar_ms, sample_at_bar_ends, simulate_ticks, tick_bars, tick_day_bounds, TickInputs,
    TickSizing,
};
use q_parity::compare::{compare_f64, MismatchKind, Policy};
use q_parity::determinism::{check_double_run_with, BitEq};
use q_parity::fixture::{load_family, Column, ColumnData, FixtureFile};
use q_parity::gate::{check_accounting, parse_pending, GateFailure, GateReport};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const BACKEND_REV: &str = include_str!("../../../BACKEND_REV");

const BOUND_TICK_KERNEL: &[&str] = &[
    "t01_sl_before_tp",
    "t02_tp_only",
    "t03_opposite_signal_no_reentry",
    "t04_long_spread_loss",
    "t05_short_spread_loss",
    "t06_end_of_stream_close",
    "t07_nan_levels",
    "t08_fractional_fixed_quantity",
    "t09_safety_margin_min_max",
    "t10_negative_capital",
    "t11_synthetic_stream",
    "t12_day_bounds",
];

const BOUND_TICK_BARS: &[&str] = &[
    "b01_last_price_m1",
    "b02_midpoint_when_no_last",
    "b03_nan_price_in_bar",
    "b04_pairwise_volume_lengths",
    "b05_interval_doubling",
    "b06_indicator_sampling_nan",
    "b07_out_of_order_ticks",
    "b08_synthetic_stream_m5",
];

#[derive(Clone, Debug, Default)]
struct CaseOutputs {
    /// Column name → values (ints encoded as f64, matching exit_rules_gate).
    columns: BTreeMap<String, Vec<f64>>,
    scalars_f64: BTreeMap<String, f64>,
    scalars_i64: BTreeMap<String, i64>,
}

impl BitEq for CaseOutputs {
    fn bit_eq(&self, other: &Self) -> Result<(), String> {
        if self.columns.len() != other.columns.len() {
            return Err(format!(
                "column count {} vs {}",
                self.columns.len(),
                other.columns.len()
            ));
        }
        for (name, a) in &self.columns {
            let b = other
                .columns
                .get(name)
                .ok_or_else(|| format!("missing column {name}"))?;
            a.bit_eq(b).map_err(|e| format!("column {name}: {e}"))?;
        }
        if self.scalars_f64.len() != other.scalars_f64.len() {
            return Err(format!(
                "scalars_f64 count {} vs {}",
                self.scalars_f64.len(),
                other.scalars_f64.len()
            ));
        }
        for (name, &a) in &self.scalars_f64 {
            let &b = other
                .scalars_f64
                .get(name)
                .ok_or_else(|| format!("missing scalar_f64 {name}"))?;
            vec![a]
                .bit_eq(&vec![b])
                .map_err(|e| format!("scalar_f64 {name}: {e}"))?;
        }
        if self.scalars_i64.len() != other.scalars_i64.len() {
            return Err(format!(
                "scalars_i64 count {} vs {}",
                self.scalars_i64.len(),
                other.scalars_i64.len()
            ));
        }
        for (name, &a) in &self.scalars_i64 {
            let &b = other
                .scalars_i64
                .get(name)
                .ok_or_else(|| format!("missing scalar_i64 {name}"))?;
            if a != b {
                return Err(format!("scalar_i64 {name}: {a} != {b}"));
            }
        }
        Ok(())
    }
}

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

fn set_float64_checksum(col: &mut serde_json::Value) {
    let values: Vec<f64> = col["values"]
        .as_array()
        .expect("values")
        .iter()
        .map(|v| v.as_f64().unwrap_or(f64::NAN))
        .collect();
    let checksum = q_parity::fixture::fnv1a64_float64(&values);
    col["bits_fnv1a64"] = serde_json::Value::String(format!("0x{checksum:016x}"));
}

fn set_int64_checksum(col: &mut serde_json::Value) {
    let values: Vec<i64> = col["values"]
        .as_array()
        .expect("values")
        .iter()
        .map(|v| v.as_i64().expect("i64"))
        .collect();
    let checksum = q_parity::fixture::fnv1a64_int64(&values);
    col["bits_fnv1a64"] = serde_json::Value::String(format!("0x{checksum:016x}"));
}

fn fixtures_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/reference")
}

fn parse_f64_col(value: &serde_json::Value, name: &str) -> Result<Vec<f64>, String> {
    let col = Column::from_json(value).map_err(|e| format!("{name}: {e}"))?;
    match col.data {
        ColumnData::Float64(v) => Ok(v),
        _ => Err(format!("{name}: expected float64")),
    }
}

fn parse_i64_col(value: &serde_json::Value, name: &str) -> Result<Vec<i64>, String> {
    let col = Column::from_json(value).map_err(|e| format!("{name}: {e}"))?;
    match col.data {
        ColumnData::Int64(v) => Ok(v),
        _ => Err(format!("{name}: expected int64")),
    }
}

fn i64_as_f64(v: &[i64]) -> Vec<f64> {
    v.iter().map(|&i| i as f64).collect()
}

fn i8_as_f64(v: &[i8]) -> Vec<f64> {
    v.iter().map(|&i| i as f64).collect()
}

fn sizing_from_json(obj: &serde_json::Value) -> Result<TickSizing, String> {
    let typ = obj
        .get("type")
        .and_then(|v| v.as_str())
        .ok_or("sizing.type missing")?;
    match typ {
        "fixed_quantity" => {
            let quantity = obj
                .get("quantity")
                .and_then(|v| v.as_f64())
                .ok_or("sizing.quantity")?;
            Ok(TickSizing::FixedQuantity { quantity })
        }
        "fixed_safety_margin" => {
            let margin_per_contract = obj
                .get("safety_margin_per_contract")
                .and_then(|v| v.as_f64())
                .ok_or("sizing.safety_margin_per_contract")?;
            let min_contracts = obj
                .get("min_contracts")
                .and_then(|v| v.as_i64())
                .ok_or("sizing.min_contracts")?;
            // Python `None` → Rust `0` (no maximum).
            let max_contracts = match obj.get("max_contracts") {
                None | Some(serde_json::Value::Null) => 0,
                Some(v) => v.as_i64().ok_or("sizing.max_contracts")?,
            };
            Ok(TickSizing::FixedSafetyMargin {
                margin_per_contract,
                min_contracts,
                max_contracts,
            })
        }
        other => Err(format!("unsupported sizing.type {other}")),
    }
}

fn replay_simulate(case: &serde_json::Value) -> Result<CaseOutputs, String> {
    let params = case.get("params").ok_or("missing params")?;
    let inputs_obj = case
        .get("inputs")
        .and_then(|v| v.as_object())
        .ok_or("missing inputs")?;

    let bid = parse_f64_col(inputs_obj.get("bid").ok_or("missing bid")?, "bid")?;
    let ask = parse_f64_col(inputs_obj.get("ask").ok_or("missing ask")?, "ask")?;
    let direction_i64 = parse_i64_col(
        inputs_obj.get("direction").ok_or("missing direction")?,
        "direction",
    )?;
    let direction: Vec<i8> = direction_i64
        .iter()
        .map(|&d| i8::try_from(d).map_err(|_| format!("direction out of i8 range: {d}")))
        .collect::<Result<_, _>>()?;
    let sl_points = parse_f64_col(
        inputs_obj.get("sl_points").ok_or("missing sl_points")?,
        "sl_points",
    )?;
    let tp_points = parse_f64_col(
        inputs_obj.get("tp_points").ok_or("missing tp_points")?,
        "tp_points",
    )?;

    let initial_capital = params
        .get("initial_capital")
        .and_then(|v| v.as_f64())
        .ok_or("params.initial_capital")?;
    let point_value = params
        .get("point_value")
        .and_then(|v| v.as_f64())
        .ok_or("params.point_value")?;
    let sizing = sizing_from_json(params.get("sizing").ok_or("params.sizing")?)?;

    let inputs = TickInputs {
        bid: &bid,
        ask: &ask,
        direction: &direction,
        sl_points: &sl_points,
        tp_points: &tp_points,
    };
    let run = simulate_ticks(&inputs, initial_capital, point_value, sizing)
        .map_err(|e| format!("simulate_ticks: {e:?}"))?;

    let mut out = CaseOutputs::default();
    out.columns
        .insert("entry_idx".into(), i64_as_f64(&run.trades.entry_idx));
    out.columns
        .insert("exit_idx".into(), i64_as_f64(&run.trades.exit_idx));
    out.columns
        .insert("entry_price".into(), run.trades.entry_price.clone());
    out.columns
        .insert("exit_price".into(), run.trades.exit_price.clone());
    out.columns
        .insert("direction".into(), i8_as_f64(&run.trades.direction));
    out.columns
        .insert("quantity".into(), run.trades.quantity.clone());
    out.columns
        .insert("exit_reason".into(), i64_as_f64(&run.trades.exit_reason));
    out.scalars_f64
        .insert("final_capital".into(), run.final_capital);
    Ok(out)
}

fn replay_day_bounds(case: &serde_json::Value) -> Result<CaseOutputs, String> {
    let inputs_obj = case
        .get("inputs")
        .and_then(|v| v.as_object())
        .ok_or("missing inputs")?;
    let time_msc = parse_i64_col(
        inputs_obj.get("time_msc").ok_or("missing time_msc")?,
        "time_msc",
    )?;
    let (starts, ends) = tick_day_bounds(&time_msc);
    let mut out = CaseOutputs::default();
    out.columns.insert("starts".into(), i64_as_f64(&starts));
    out.columns.insert("ends".into(), i64_as_f64(&ends));
    Ok(out)
}

fn replay_tick_bars(case: &serde_json::Value) -> Result<CaseOutputs, String> {
    let inputs_obj = case
        .get("inputs")
        .and_then(|v| v.as_object())
        .ok_or("missing inputs")?;
    let time_msc = parse_i64_col(
        inputs_obj.get("time_msc").ok_or("missing time_msc")?,
        "time_msc",
    )?;
    let bid = parse_f64_col(inputs_obj.get("bid").ok_or("missing bid")?, "bid")?;
    let ask = parse_f64_col(inputs_obj.get("ask").ok_or("missing ask")?, "ask")?;
    let last = parse_f64_col(inputs_obj.get("last").ok_or("missing last")?, "last")?;
    let volume = parse_f64_col(inputs_obj.get("volume").ok_or("missing volume")?, "volume")?;
    let bar_ms = inputs_obj
        .get("bar_ms")
        .and_then(|v| v.as_i64())
        .ok_or("inputs.bar_ms scalar")?;

    let bars = tick_bars(&time_msc, &bid, &ask, &last, &volume, bar_ms)
        .map_err(|e| format!("tick_bars: {e:?}"))?;

    let mut out = CaseOutputs::default();
    out.columns
        .insert("open_msc".into(), i64_as_f64(&bars.open_msc));
    out.columns.insert("open".into(), bars.open);
    out.columns.insert("high".into(), bars.high);
    out.columns.insert("low".into(), bars.low);
    out.columns.insert("close".into(), bars.close);
    out.columns
        .insert("volume".into(), i64_as_f64(&bars.volume));
    out.columns
        .insert("tick_start".into(), i64_as_f64(&bars.tick_start));
    out.columns
        .insert("tick_end".into(), i64_as_f64(&bars.tick_end));
    Ok(out)
}

fn replay_resolve_bar_ms(case: &serde_json::Value) -> Result<CaseOutputs, String> {
    let inputs_obj = case
        .get("inputs")
        .and_then(|v| v.as_object())
        .ok_or("missing inputs")?;
    let base_bar_ms = inputs_obj
        .get("base_bar_ms")
        .and_then(|v| v.as_i64())
        .ok_or("inputs.base_bar_ms")?;
    let span_msc = inputs_obj
        .get("span_msc")
        .and_then(|v| v.as_i64())
        .ok_or("inputs.span_msc")?;
    let mut out = CaseOutputs::default();
    out.scalars_i64
        .insert("bar_ms".into(), resolve_bar_ms(base_bar_ms, span_msc));
    Ok(out)
}

fn replay_sample_at_bar_ends(case: &serde_json::Value) -> Result<CaseOutputs, String> {
    let inputs_obj = case
        .get("inputs")
        .and_then(|v| v.as_object())
        .ok_or("missing inputs")?;
    let series = parse_f64_col(inputs_obj.get("series").ok_or("missing series")?, "series")?;
    let tick_end = parse_i64_col(
        inputs_obj.get("tick_end").ok_or("missing tick_end")?,
        "tick_end",
    )?;
    let sampled = sample_at_bar_ends(&series, &tick_end);
    let mut out = CaseOutputs::default();
    out.columns.insert("values".into(), sampled);
    Ok(out)
}

fn replay_tick_kernel_case(case: &serde_json::Value) -> Result<CaseOutputs, String> {
    let expected = case
        .get("expected")
        .and_then(|v| v.as_object())
        .ok_or("missing expected")?;
    if expected.contains_key("starts") {
        replay_day_bounds(case)
    } else {
        replay_simulate(case)
    }
}

fn replay_tick_bars_case(case: &serde_json::Value) -> Result<CaseOutputs, String> {
    let operation = case
        .get("operation")
        .and_then(|v| v.as_str())
        .unwrap_or("tick_bars");
    match operation {
        "tick_bars" => replay_tick_bars(case),
        "resolve_bar_ms" => replay_resolve_bar_ms(case),
        "sample_at_bar_ends" => replay_sample_at_bar_ends(case),
        other => Err(format!("unknown tick_bars operation {other}")),
    }
}

fn compare_case(
    file: &FixtureFile,
    case: &serde_json::Value,
    actual: &CaseOutputs,
) -> Result<(), Box<GateFailure>> {
    let case_id = case
        .get("case_id")
        .and_then(|v| v.as_str())
        .unwrap_or("*")
        .to_string();
    let expected = case
        .get("expected")
        .and_then(|v| v.as_object())
        .ok_or_else(|| {
            Box::new(GateFailure::UnexpectedError {
                function_id: file.fixture_id.clone(),
                case_id: case_id.clone(),
                message: "missing expected".into(),
            })
        })?;

    let policy = Policy::Exact;
    for (name, exp_val) in expected {
        if name == "final_capital" {
            let exp = exp_val.as_f64().ok_or_else(|| {
                Box::new(GateFailure::UnexpectedError {
                    function_id: file.fixture_id.clone(),
                    case_id: case_id.clone(),
                    message: "final_capital: expected f64 scalar".into(),
                })
            })?;
            let act = *actual.scalars_f64.get(name).ok_or_else(|| {
                Box::new(GateFailure::UnexpectedError {
                    function_id: file.fixture_id.clone(),
                    case_id: case_id.clone(),
                    message: format!("missing actual scalar {name}"),
                })
            })?;
            if let Err((index, kind)) = compare_f64(&policy, &[exp], &[act]) {
                return Err(Box::new(GateFailure::Golden {
                    function_id: file.fixture_id.clone(),
                    case_id: case_id.clone(),
                    mismatch: q_parity::compare::Mismatch {
                        output: name.clone(),
                        index,
                        expected: format!("{exp}"),
                        actual: format!("{act}"),
                        kind,
                    },
                }));
            }
            continue;
        }

        if name == "bar_ms" {
            let exp = exp_val.as_i64().ok_or_else(|| {
                Box::new(GateFailure::UnexpectedError {
                    function_id: file.fixture_id.clone(),
                    case_id: case_id.clone(),
                    message: "bar_ms: expected i64 scalar".into(),
                })
            })?;
            let act = *actual.scalars_i64.get(name).ok_or_else(|| {
                Box::new(GateFailure::UnexpectedError {
                    function_id: file.fixture_id.clone(),
                    case_id: case_id.clone(),
                    message: format!("missing actual scalar {name}"),
                })
            })?;
            if exp != act {
                return Err(Box::new(GateFailure::Golden {
                    function_id: file.fixture_id.clone(),
                    case_id: case_id.clone(),
                    mismatch: q_parity::compare::Mismatch {
                        output: name.clone(),
                        index: 0,
                        expected: format!("{exp}"),
                        actual: format!("{act}"),
                        kind: q_parity::compare::MismatchKind::Int64,
                    },
                }));
            }
            continue;
        }

        let actual_col = actual.columns.get(name).ok_or_else(|| {
            Box::new(GateFailure::UnexpectedError {
                function_id: file.fixture_id.clone(),
                case_id: case_id.clone(),
                message: format!("missing actual column {name}"),
            })
        })?;
        let exp_col = Column::from_json(exp_val).map_err(|e| {
            Box::new(GateFailure::UnexpectedError {
                function_id: file.fixture_id.clone(),
                case_id: case_id.clone(),
                message: format!("{name}: {e}"),
            })
        })?;
        let exp_f64: Vec<f64> = match exp_col.data {
            ColumnData::Float64(v) => v,
            ColumnData::Int64(v) => i64_as_f64(&v),
        };
        if let Err((index, kind)) = compare_f64(&policy, &exp_f64, actual_col) {
            return Err(Box::new(GateFailure::Golden {
                function_id: file.fixture_id.clone(),
                case_id: case_id.clone(),
                mismatch: q_parity::compare::Mismatch {
                    output: name.clone(),
                    index,
                    expected: String::new(),
                    actual: String::new(),
                    kind,
                },
            }));
        }
    }
    Ok(())
}

fn run_family(
    family: &str,
    bound: &[&str],
    replay: fn(&serde_json::Value) -> Result<CaseOutputs, String>,
    failures: &mut Vec<GateFailure>,
) {
    let root = fixtures_root();
    let files = load_family(&root, family).unwrap_or_else(|e| panic!("load_family({family}): {e}"));
    let pending = parse_pending("").expect("parse_pending");
    failures.extend(check_accounting(
        &files,
        bound,
        &pending,
        BACKEND_REV.trim(),
    ));

    for file in &files {
        for case in &file.cases {
            let case_id = case
                .get("case_id")
                .and_then(|v| v.as_str())
                .unwrap_or("*")
                .to_string();
            match replay(case) {
                Ok(out) => {
                    if let Err(f) = compare_case(file, case, &out) {
                        failures.push(*f);
                    }
                }
                Err(message) => failures.push(GateFailure::UnexpectedError {
                    function_id: file.fixture_id.clone(),
                    case_id,
                    message,
                }),
            }
        }
    }
}

#[test]
fn tick_reference_gate() {
    let mut failures = Vec::new();
    run_family(
        "tick_kernel",
        BOUND_TICK_KERNEL,
        replay_tick_kernel_case,
        &mut failures,
    );
    run_family(
        "tick_bars",
        BOUND_TICK_BARS,
        replay_tick_bars_case,
        &mut failures,
    );

    let mut bound: Vec<String> = BOUND_TICK_KERNEL
        .iter()
        .chain(BOUND_TICK_BARS.iter())
        .map(|s| (*s).to_string())
        .collect();
    bound.sort();

    let report = GateReport {
        bound,
        pending: Vec::new(),
        failures,
    };
    println!("{}", report.summary());
    report.assert_passed();
}

#[test]
fn tick_double_run() {
    let root = fixtures_root();
    for family in ["tick_kernel", "tick_bars"] {
        let files =
            load_family(&root, family).unwrap_or_else(|e| panic!("load_family({family}): {e}"));
        let replay: fn(&serde_json::Value) -> Result<CaseOutputs, String> = match family {
            "tick_kernel" => replay_tick_kernel_case,
            "tick_bars" => replay_tick_bars_case,
            _ => unreachable!(),
        };
        for file in &files {
            for case in &file.cases {
                let case_id = case.get("case_id").and_then(|v| v.as_str()).unwrap_or("*");
                check_double_run_with(|| replay(case).expect("replay")).unwrap_or_else(|e| {
                    panic!("{} / {case_id} nondeterministic: {e}", file.fixture_id)
                });
            }
        }
    }
}

#[test]
fn tick_negative_controls() {
    let root = fixtures_root();
    let kernel_files = load_family(&root, "tick_kernel").expect("load tick_kernel");
    let bars_files = load_family(&root, "tick_bars").expect("load tick_bars");
    let t01 = kernel_files
        .iter()
        .find(|f| f.fixture_id == "t01_sl_before_tp")
        .expect("t01_sl_before_tp");
    let b01 = bars_files
        .iter()
        .find(|f| f.fixture_id == "b01_last_price_m1")
        .expect("b01_last_price_m1");

    // 1) One ULP on entry_price (checksum refreshed so compare sees Bits).
    {
        let mut case = t01.cases[0].clone();
        let price = case
            .pointer_mut("/expected/entry_price/values/0")
            .expect("entry_price");
        let bits = price.as_f64().unwrap().to_bits().wrapping_add(1);
        *price = serde_json::Number::from_f64(f64::from_bits(bits))
            .map(serde_json::Value::Number)
            .unwrap();
        set_float64_checksum(&mut case["expected"]["entry_price"]);
        let mut mutated = clone_fixture(t01);
        mutated.cases[0] = case;
        let out = replay_tick_kernel_case(&mutated.cases[0]).unwrap();
        let err = compare_case(&mutated, &mutated.cases[0], &out).unwrap_err();
        match *err {
            GateFailure::Golden { mismatch, .. } if mismatch.kind == MismatchKind::Bits => {}
            other => panic!("expected Bits on entry_price ULP, got {other:?}"),
        }
    }

    // 2) One exit tick moved by +1.
    {
        let mut case = t01.cases[0].clone();
        let idx = case
            .pointer_mut("/expected/exit_idx/values/0")
            .expect("exit_idx");
        *idx = serde_json::json!(idx.as_i64().unwrap() + 1);
        set_int64_checksum(&mut case["expected"]["exit_idx"]);
        let mut mutated = clone_fixture(t01);
        mutated.cases[0] = case;
        let out = replay_tick_kernel_case(&mutated.cases[0]).unwrap();
        assert!(
            compare_case(&mutated, &mutated.cases[0], &out).is_err(),
            "exit_idx +1 should fail compare"
        );
    }

    // 3) One ULP on final_capital (scalar, no checksum).
    {
        let mut case = t01.cases[0].clone();
        let cap = case
            .pointer_mut("/expected/final_capital")
            .expect("final_capital");
        let bits = cap.as_f64().unwrap().to_bits().wrapping_add(1);
        *cap = serde_json::Number::from_f64(f64::from_bits(bits))
            .map(serde_json::Value::Number)
            .unwrap();
        let mut mutated = clone_fixture(t01);
        mutated.cases[0] = case;
        let out = replay_tick_kernel_case(&mutated.cases[0]).unwrap();
        let err = compare_case(&mutated, &mutated.cases[0], &out).unwrap_err();
        match *err {
            GateFailure::Golden { mismatch, .. } if mismatch.kind == MismatchKind::Bits => {}
            other => panic!("expected Bits on final_capital ULP, got {other:?}"),
        }
    }

    // 4) One bar volume changed by +1.
    {
        let mut case = b01.cases[0].clone();
        let vol = case
            .pointer_mut("/expected/volume/values/0")
            .expect("volume");
        *vol = serde_json::json!(vol.as_i64().unwrap() + 1);
        set_int64_checksum(&mut case["expected"]["volume"]);
        let mut mutated = clone_fixture(b01);
        mutated.cases[0] = case;
        let out = replay_tick_bars_case(&mutated.cases[0]).unwrap();
        assert!(
            compare_case(&mutated, &mutated.cases[0], &out).is_err(),
            "volume +1 should fail compare"
        );
    }

    // 5) Checksum mismatch: value changed without updating bits_fnv1a64.
    {
        let mut case = t01.cases[0].clone();
        let price = case
            .pointer_mut("/expected/entry_price/values/0")
            .expect("entry_price");
        let bits = price.as_f64().unwrap().to_bits().wrapping_add(1);
        *price = serde_json::Number::from_f64(f64::from_bits(bits))
            .map(serde_json::Value::Number)
            .unwrap();
        // Leave bits_fnv1a64 stale.
        let mut mutated = clone_fixture(t01);
        mutated.cases[0] = case;
        let out = replay_tick_kernel_case(&mutated.cases[0]).unwrap();
        let err = compare_case(&mutated, &mutated.cases[0], &out).unwrap_err();
        match *err {
            GateFailure::Golden { .. } | GateFailure::UnexpectedError { .. } => {}
            other => panic!("expected checksum/compare failure, got {other:?}"),
        }
    }

    // 6) Unaccounted fixture file.
    {
        let tmp = tempfile_family_copy(&root, "tick_kernel");
        std::fs::write(
            tmp.join("tick_kernel/zz_extra.json"),
            r#"{"format":"q-core-reference-fixture/1","family":"tick_kernel","fixture_id":"zz_extra","policy":{"kind":"exact"},"provenance":{"backend_rev":"x"},"cases":[]}"#,
        )
        .unwrap();
        let files = load_family(&tmp, "tick_kernel").expect("load");
        let pending = parse_pending("").unwrap();
        let failures = check_accounting(&files, BOUND_TICK_KERNEL, &pending, BACKEND_REV.trim());
        assert!(
            failures
                .iter()
                .any(|f| matches!(f, GateFailure::Unaccounted { .. })),
            "expected Unaccounted, got {failures:?}"
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }
}

fn tempfile_family_copy(root: &Path, family: &str) -> PathBuf {
    let tmp =
        std::env::temp_dir().join(format!("q029-tick-gate-{}-{}", family, std::process::id()));
    let dest = tmp.join(family);
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&dest).unwrap();
    for entry in std::fs::read_dir(root.join(family)).unwrap() {
        let entry = entry.unwrap();
        std::fs::copy(entry.path(), dest.join(entry.file_name())).unwrap();
    }
    tmp
}
