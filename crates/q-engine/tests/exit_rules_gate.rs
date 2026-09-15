#![forbid(unsafe_code)]

use q_engine::{
    ExitBook, ExitDecision, ExitInputs, ExitParams, ExitRuleId, ExitRuleSet, OpenPosition,
    ParamValue, PositionKey, RuleState, Side,
};
use q_parity::compare::{compare_f64, MismatchKind, Policy};
use q_parity::determinism::{check_double_run_with, BitEq};
use q_parity::fixture::{load_family, Column, ColumnData, FixtureFile};
use q_parity::gate::{check_accounting, parse_pending, GateFailure, GateReport};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const BACKEND_REV: &str = include_str!("../../../BACKEND_REV");

const BOUND_SCENARIOS: &[&str] = &[
    "r01_fixed_sl",
    "r02_atr_sl",
    "r03_fixed_tp",
    "r04_atr_tp",
    "r05_trailing",
    "r06_chandelier",
    "r07_breakeven",
    "r08_psar",
    "r09_profit_target_ratchet",
    "r10_time_stop",
    "r11_donchian_stop",
    "c01_all_rules",
    "c02_close_only",
    "c03_multi_positions_reuse",
    "c04_psar_cap",
    "c05_time_stop_one",
];

const STATE_FLOATS: &[&str] = &[
    "trailing_extreme",
    "chandelier_peak",
    "psar_sar",
    "psar_ep",
    "psar_af",
    "psar_prior",
    "psar_prior_prior",
    "ratchet",
];

const STATE_INTS: &[&str] = &["breakeven_armed", "psar_present", "time_stop_bars"];

#[derive(Clone, Debug)]
struct PosInterval {
    key: u64,
    side: Side,
    entry: f64,
    first: usize,
    last: usize,
}

#[derive(Clone, Debug)]
struct ExitTrace {
    /// column name -> values (float-encoded; ints as f64)
    columns: BTreeMap<String, Vec<f64>>,
}

impl BitEq for ExitTrace {
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
        Ok(())
    }
}

fn fixtures_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/reference")
}

fn parse_f64_values(value: &serde_json::Value, name: &str) -> Result<Vec<f64>, String> {
    // Prefer full column decode (checksum); fall back to raw values for truncated prefixes.
    match Column::from_json(value) {
        Ok(col) => match col.data {
            ColumnData::Float64(v) => Ok(v),
            _ => Err(format!("{name}: expected float64")),
        },
        Err(_) => {
            let values = value
                .get("values")
                .and_then(|v| v.as_array())
                .ok_or_else(|| format!("{name}: missing values"))?;
            Ok(values
                .iter()
                .map(|v| {
                    if v.is_null() {
                        Ok(f64::NAN)
                    } else {
                        v.as_f64()
                            .ok_or_else(|| format!("{name}: non-float value {v}"))
                    }
                })
                .collect::<Result<Vec<_>, _>>()?)
        }
    }
}

fn parse_i64_values(value: &serde_json::Value, name: &str) -> Result<Vec<i64>, String> {
    match Column::from_json(value) {
        Ok(col) => match col.data {
            ColumnData::Int64(v) => Ok(v),
            _ => Err(format!("{name}: expected int64")),
        },
        Err(_) => {
            let values = value
                .get("values")
                .and_then(|v| v.as_array())
                .ok_or_else(|| format!("{name}: missing values"))?;
            values
                .iter()
                .map(|v| {
                    v.as_i64()
                        .ok_or_else(|| format!("{name}: non-int value {v}"))
                })
                .collect()
        }
    }
}

fn params_from_json(obj: &serde_json::Value) -> Result<ExitParams, String> {
    let map = obj
        .as_object()
        .ok_or_else(|| "params must be object".to_string())?;
    let mut pairs = Vec::new();
    for (k, v) in map {
        let pv = if let Some(b) = v.as_bool() {
            ParamValue::Bool(b)
        } else if let Some(i) = v.as_i64() {
            ParamValue::Int(i)
        } else if let Some(f) = v.as_f64() {
            // Prefer Int when JSON number is integral (max_bars, periods).
            if f.fract() == 0.0 && f.is_finite() && f >= i64::MIN as f64 && f <= i64::MAX as f64 {
                ParamValue::Int(f as i64)
            } else {
                ParamValue::Float(f)
            }
        } else {
            return Err(format!("unsupported param {k}: {v}"));
        };
        pairs.push((k.as_str(), pv));
    }
    ExitParams::from_pairs(pairs).map_err(|e| e.to_string())
}

fn schedule_from_json(obj: &serde_json::Value) -> Result<Vec<PosInterval>, String> {
    let keys = parse_i64_values(obj.get("key").ok_or("missing schedule.key")?, "key")?;
    let sides = parse_i64_values(obj.get("side").ok_or("missing schedule.side")?, "side")?;
    let entries = parse_f64_values(
        obj.get("entry_price")
            .ok_or("missing schedule.entry_price")?,
        "entry_price",
    )?;
    let firsts = parse_i64_values(
        obj.get("first_bar").ok_or("missing schedule.first_bar")?,
        "first_bar",
    )?;
    let lasts = parse_i64_values(
        obj.get("last_bar").ok_or("missing schedule.last_bar")?,
        "last_bar",
    )?;
    let n = keys.len();
    if [sides.len(), entries.len(), firsts.len(), lasts.len()]
        .iter()
        .any(|&l| l != n)
    {
        return Err("schedule column length mismatch".into());
    }
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let side = match sides[i] {
            0 => Side::Long,
            1 => Side::Short,
            other => return Err(format!("invalid side {other}")),
        };
        out.push(PosInterval {
            key: keys[i] as u64,
            side,
            entry: entries[i],
            first: firsts[i] as usize,
            last: lasts[i] as usize,
        });
    }
    Ok(out)
}

fn encode_state(state: &RuleState, enabled: &[ExitRuleId]) -> BTreeMap<&'static str, f64> {
    let has = |id: ExitRuleId| enabled.contains(&id);
    let mut m = BTreeMap::new();
    m.insert(
        "trailing_extreme",
        state.trailing_extreme.unwrap_or(f64::NAN),
    );
    m.insert("chandelier_peak", state.chandelier_peak.unwrap_or(f64::NAN));
    // Python only writes armed=True; absent → -1. Encode false as -1.
    m.insert(
        "breakeven_armed",
        if !has(ExitRuleId::Breakeven) {
            -1.0
        } else if state.breakeven_armed {
            1.0
        } else {
            -1.0
        },
    );
    match &state.psar {
        Some(ps) if has(ExitRuleId::ParabolicSar) => {
            m.insert("psar_present", 1.0);
            m.insert("psar_sar", ps.sar);
            m.insert("psar_ep", ps.ep);
            m.insert("psar_af", ps.af);
            m.insert("psar_prior", ps.prior);
            m.insert("psar_prior_prior", ps.prior_prior.unwrap_or(f64::NAN));
        }
        _ => {
            m.insert("psar_present", -1.0);
            m.insert("psar_sar", f64::NAN);
            m.insert("psar_ep", f64::NAN);
            m.insert("psar_af", f64::NAN);
            m.insert("psar_prior", f64::NAN);
            m.insert("psar_prior_prior", f64::NAN);
        }
    }
    m.insert("ratchet", state.ratchet.unwrap_or(f64::NAN));
    m.insert(
        "time_stop_bars",
        if !has(ExitRuleId::TimeStop) || state.time_stop_bars == 0 {
            -1.0
        } else {
            state.time_stop_bars as f64
        },
    );
    m
}

fn open_at(schedule: &[PosInterval], bar: usize) -> Vec<OpenPosition> {
    schedule
        .iter()
        .filter(|p| p.first <= bar && bar <= p.last)
        .map(|p| OpenPosition {
            key: PositionKey(p.key),
            side: p.side,
            entry_price: p.entry,
        })
        .collect()
}

fn replay_case(case: &serde_json::Value) -> Result<ExitTrace, String> {
    let params = params_from_json(case.get("params").ok_or("missing params")?)?;
    let rules = ExitRuleSet::new(params);
    let schedule = schedule_from_json(case.get("schedule").ok_or("missing schedule")?)?;
    let inputs_obj = case
        .get("inputs")
        .and_then(|v| v.as_object())
        .ok_or("missing inputs")?;

    let close = parse_f64_values(inputs_obj.get("close").ok_or("missing close")?, "close")?;
    let high = inputs_obj
        .get("high")
        .map(|v| parse_f64_values(v, "high"))
        .transpose()?;
    let low = inputs_obj
        .get("low")
        .map(|v| parse_f64_values(v, "low"))
        .transpose()?;

    let named_owned: BTreeMap<String, Vec<f64>> = inputs_obj
        .iter()
        .filter(|(k, _)| k.starts_with("atr_") || k.starts_with("donchian_"))
        .map(|(k, v)| parse_f64_values(v, k).map(|col| (k.clone(), col)))
        .collect::<Result<_, _>>()?;

    let exit_inputs = ExitInputs::from_slices(
        &close,
        high.as_deref(),
        low.as_deref(),
        &|name| named_owned.get(name).map(|v| v.as_slice()),
        &rules,
    )
    .map_err(|e| e.to_string())?;

    let keys: Vec<u64> = {
        let mut k: Vec<u64> = schedule.iter().map(|p| p.key).collect();
        k.sort_unstable();
        k.dedup();
        k
    };

    let n = close.len();
    let mut columns: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    for &k in &keys {
        columns.insert(format!("exit_code_{k}"), vec![-1.0; n]);
        for name in STATE_FLOATS {
            columns.insert(format!("{name}_{k}"), vec![f64::NAN; n]);
        }
        for name in STATE_INTS {
            columns.insert(format!("{name}_{k}"), vec![-1.0; n]);
        }
    }

    let enabled = rules.enabled().to_vec();
    let mut book = ExitBook::new(rules);
    for bar in 0..n {
        let positions = open_at(&schedule, bar);
        let decisions = book.evaluate(&positions, &exit_inputs, bar);
        let reason_by_key: BTreeMap<u64, ExitRuleId> = decisions
            .into_iter()
            .map(|ExitDecision { key, rule }| (key.0, rule))
            .collect();

        for pos in &positions {
            let k = pos.key.0;
            let code = reason_by_key
                .get(&k)
                .map(|r| *r as u8 as f64)
                .unwrap_or(-1.0);
            columns.get_mut(&format!("exit_code_{k}")).unwrap()[bar] = code;
            let encoded = encode_state(book.state(pos.key).unwrap(), &enabled);
            for name in STATE_FLOATS {
                columns.get_mut(&format!("{name}_{k}")).unwrap()[bar] = encoded[name];
            }
            for name in STATE_INTS {
                columns.get_mut(&format!("{name}_{k}")).unwrap()[bar] = encoded[name];
            }
        }
    }

    Ok(ExitTrace { columns })
}

fn replay_file(file: &FixtureFile) -> Result<ExitTrace, String> {
    let case = file.cases.first().ok_or("no cases")?;
    replay_case(case)
}

fn compare_trace(file: &FixtureFile, trace: &ExitTrace) -> Result<(), Box<GateFailure>> {
    let case = &file.cases[0];
    let expected = case
        .get("expected")
        .and_then(|v| v.as_object())
        .ok_or_else(|| {
            Box::new(GateFailure::UnexpectedError {
                function_id: file.fixture_id.clone(),
                case_id: case
                    .get("case_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("*")
                    .to_string(),
                message: "missing expected".into(),
            })
        })?;

    let policy = Policy::Exact;
    for (name, exp_val) in expected {
        let actual = trace.columns.get(name).ok_or_else(|| {
            Box::new(GateFailure::UnexpectedError {
                function_id: file.fixture_id.clone(),
                case_id: "*".into(),
                message: format!("missing actual column {name}"),
            })
        })?;
        let exp_col = Column::from_json(exp_val).map_err(|e| {
            Box::new(GateFailure::UnexpectedError {
                function_id: file.fixture_id.clone(),
                case_id: "*".into(),
                message: format!("{name}: {e}"),
            })
        })?;
        let exp_f64: Vec<f64> = match exp_col.data {
            ColumnData::Float64(v) => v,
            ColumnData::Int64(v) => v.into_iter().map(|i| i as f64).collect(),
        };
        if let Err((index, kind)) = compare_f64(&policy, &exp_f64, actual) {
            return Err(Box::new(GateFailure::Golden {
                function_id: file.fixture_id.clone(),
                case_id: case
                    .get("case_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("*")
                    .to_string(),
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

fn strip_checksums(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(map) => {
            map.remove("bits_fnv1a64");
            for v in map.values_mut() {
                strip_checksums(v);
            }
        }
        serde_json::Value::Array(arr) => {
            for v in arr {
                strip_checksums(v);
            }
        }
        _ => {}
    }
}

fn prefix_causal_at(case: &serde_json::Value, lengths: &[usize]) -> Result<(), String> {
    let full = replay_case(case)?;
    let n = full
        .columns
        .values()
        .next()
        .map(|v| v.len())
        .ok_or("empty trace")?;
    for &len in lengths {
        let len = len.min(n);
        let mut truncated = case.clone();
        strip_checksums(&mut truncated);
        let inputs = truncated
            .get_mut("inputs")
            .and_then(|v| v.as_object_mut())
            .ok_or("inputs")?;
        for (_k, v) in inputs.iter_mut() {
            if let Some(values) = v.get_mut("values").and_then(|x| x.as_array_mut()) {
                values.truncate(len);
            }
        }
        if let Some(last) = truncated.pointer_mut("/schedule/last_bar/values") {
            if let Some(arr) = last.as_array_mut() {
                for v in arr {
                    if let Some(i) = v.as_i64() {
                        *v = serde_json::json!(i.min(len.saturating_sub(1) as i64));
                    }
                }
            }
        }
        let prefix = replay_case(&truncated)?;
        for (name, full_col) in &full.columns {
            let pref = prefix
                .columns
                .get(name)
                .ok_or_else(|| format!("prefix missing {name}"))?;
            if pref.len() != len {
                return Err(format!("{name}: prefix len {} != {len}", pref.len()));
            }
            for i in 0..len {
                if full_col[i].to_bits() != pref[i].to_bits() {
                    return Err(format!(
                        "non-causal {name}[{i}] full={:?} prefix={:?} at len={len}",
                        full_col[i], pref[i]
                    ));
                }
            }
        }
    }
    Ok(())
}

#[test]
fn exit_rules_reference_gate() {
    let root = fixtures_root();
    let files = load_family(&root, "exit_rules").expect("load_family");
    let pending = parse_pending("").expect("parse_pending");
    let mut failures = check_accounting(&files, BOUND_SCENARIOS, &pending, BACKEND_REV.trim());

    for file in &files {
        match replay_file(file) {
            Ok(trace) => {
                if let Err(f) = compare_trace(file, &trace) {
                    failures.push(*f);
                }
            }
            Err(message) => failures.push(GateFailure::UnexpectedError {
                function_id: file.fixture_id.clone(),
                case_id: "*".to_string(),
                message,
            }),
        }
    }

    let report = GateReport {
        bound: BOUND_SCENARIOS.iter().map(|s| (*s).to_string()).collect(),
        pending: Vec::new(),
        failures,
    };
    println!("{}", report.summary());
    report.assert_passed();
}

#[test]
fn exit_rules_double_run() {
    let root = fixtures_root();
    let files = load_family(&root, "exit_rules").expect("load_family");
    for file in &files {
        check_double_run_with(|| replay_file(file).expect("replay"))
            .unwrap_or_else(|e| panic!("{} nondeterministic: {e}", file.fixture_id));
    }
}

#[test]
fn exit_rules_prefix_causality() {
    let root = fixtures_root();
    let files = load_family(&root, "exit_rules").expect("load_family");
    for file in &files {
        let case = &file.cases[0];
        let n = parse_f64_values(&case["inputs"]["close"], "close")
            .expect("close")
            .len();
        let lengths = [1usize, 17, 80, n];
        prefix_causal_at(case, &lengths).unwrap_or_else(|e| {
            panic!("{} prefix causality: {e}", file.fixture_id);
        });
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

#[test]
fn exit_rules_negative_controls() {
    let root = fixtures_root();
    let files = load_family(&root, "exit_rules").expect("load_family");
    let file = files
        .iter()
        .find(|f| f.fixture_id == "r05_trailing")
        .expect("r05_trailing");

    // 1) One ULP on a state float (first finite trailing extreme).
    {
        let mut case = file.cases[0].clone();
        let values = case["expected"]["trailing_extreme_1"]["values"]
            .as_array_mut()
            .expect("trailing_extreme_1");
        let idx = values
            .iter()
            .position(|v| v.as_f64().is_some())
            .expect("finite trailing extreme");
        let bits = values[idx].as_f64().unwrap().to_bits().wrapping_add(1);
        values[idx] = serde_json::Number::from_f64(f64::from_bits(bits))
            .map(serde_json::Value::Number)
            .unwrap();
        let fvalues: Vec<f64> = values
            .iter()
            .map(|v| v.as_f64().unwrap_or(f64::NAN))
            .collect();
        let checksum = q_parity::fixture::fnv1a64_float64(&fvalues);
        case["expected"]["trailing_extreme_1"]["bits_fnv1a64"] =
            serde_json::Value::String(format!("0x{checksum:016x}"));
        let mut mutated = clone_fixture(file);
        mutated.cases[0] = case;
        let trace = replay_file(&mutated).unwrap();
        let err = compare_trace(&mutated, &trace).unwrap_err();
        match *err {
            GateFailure::Golden { mismatch, .. } if mismatch.kind == MismatchKind::Bits => {}
            other => panic!("expected Bits, got {other:?}"),
        }
    }

    // 2) Move an exit one bar later.
    {
        let mut case = file.cases[0].clone();
        let values = case["expected"]["exit_code_1"]["values"]
            .as_array_mut()
            .unwrap();
        let mut first = None;
        for (i, v) in values.iter().enumerate() {
            if v.as_f64().unwrap() >= 0.0 {
                first = Some(i);
                break;
            }
        }
        let i = first.expect("an exit");
        let code = values[i].clone();
        values[i] = serde_json::json!(-1.0);
        if i + 1 < values.len() {
            values[i + 1] = code;
        }
        let fvalues: Vec<f64> = values.iter().map(|v| v.as_f64().unwrap()).collect();
        let checksum = q_parity::fixture::fnv1a64_float64(&fvalues);
        case["expected"]["exit_code_1"]["bits_fnv1a64"] =
            serde_json::Value::String(format!("0x{checksum:016x}"));
        let mut mutated = clone_fixture(file);
        mutated.cases[0] = case;
        let trace = replay_file(&mutated).unwrap();
        assert!(compare_trace(&mutated, &trace).is_err());
    }

    // 3) Wrong exit reason.
    {
        let mut case = file.cases[0].clone();
        let values = case["expected"]["exit_code_1"]["values"]
            .as_array_mut()
            .unwrap();
        for v in values.iter_mut() {
            if v.as_f64().unwrap() >= 0.0 {
                *v = serde_json::json!(0.0); // FixedStopLoss
                break;
            }
        }
        let fvalues: Vec<f64> = values.iter().map(|v| v.as_f64().unwrap()).collect();
        let checksum = q_parity::fixture::fnv1a64_float64(&fvalues);
        case["expected"]["exit_code_1"]["bits_fnv1a64"] =
            serde_json::Value::String(format!("0x{checksum:016x}"));
        let mut mutated = clone_fixture(file);
        mutated.cases[0] = case;
        let trace = replay_file(&mutated).unwrap();
        assert!(compare_trace(&mutated, &trace).is_err());
    }

    // 4) Checksum mismatch without updating bits.
    {
        let mut case = file.cases[0].clone();
        let val = case.pointer_mut("/expected/exit_code_1/values/0").unwrap();
        *val = serde_json::json!(42.0);
        let mut mutated = clone_fixture(file);
        mutated.cases[0] = case;
        let trace = replay_file(&mutated).unwrap();
        let err = compare_trace(&mutated, &trace).unwrap_err();
        // compare_f64 fail-closes on checksum before bits when checksum present
        match *err {
            GateFailure::Golden { .. } | GateFailure::UnexpectedError { .. } => {}
            other => panic!("expected failure, got {other:?}"),
        }
    }

    // 5) Unaccounted fixture file.
    {
        let tmp = tempfile_exit_rules_copy(&root);
        std::fs::write(
            tmp.join("exit_rules/zz_extra.json"),
            r#"{"format":"q-core-reference-fixture/1","family":"exit_rules","fixture_id":"zz_extra","policy":{"kind":"exact"},"provenance":{"backend_rev":"x"},"cases":[]}"#,
        )
        .unwrap();
        let files = load_family(&tmp, "exit_rules").expect("load");
        let pending = parse_pending("").unwrap();
        let failures = check_accounting(&files, BOUND_SCENARIOS, &pending, BACKEND_REV.trim());
        assert!(
            failures
                .iter()
                .any(|f| matches!(f, GateFailure::Unaccounted { .. })),
            "expected Unaccounted, got {failures:?}"
        );
    }
}

fn tempfile_exit_rules_copy(root: &Path) -> PathBuf {
    let tmp = std::env::temp_dir().join(format!("q026-exit-rules-{}", std::process::id()));
    std::fs::create_dir_all(tmp.join("exit_rules")).unwrap();
    for entry in std::fs::read_dir(root.join("exit_rules")).unwrap() {
        let entry = entry.unwrap();
        let dest = tmp.join("exit_rules").join(entry.file_name());
        std::fs::copy(entry.path(), dest).unwrap();
    }
    tmp
}
