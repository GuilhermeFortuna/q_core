#![forbid(unsafe_code)]

use q_buffers::{BarColumns, RollingBarWindow, TimeLabel, VolumeSet};
use q_parity::compare::{compare_f64, Mismatch, MismatchKind};
use q_parity::determinism::{check_double_run_with, BitEq};
use q_parity::fixture::{load_family, Column, ColumnData, FixtureFile};
use q_parity::gate::{check_accounting, parse_pending, GateFailure, GateReport};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

const BACKEND_REV: &str = include_str!("../../../BACKEND_REV");
const BOUND_SCENARIOS: &[&str] = &[
    "s01_seed",
    "s02_append",
    "s03_redeliver",
    "s04_overlap",
    "s05_late_full",
    "s06_gap_fill",
    "s07_unsorted_duplicates",
    "s08_empty",
    "s09_bound_one",
];

#[derive(Clone, Debug)]
struct WindowTrace {
    times: Vec<Vec<i64>>,
    prices: Vec<Vec<f64>>, // flattened open,high,low,close per step
}

impl BitEq for WindowTrace {
    fn bit_eq(&self, other: &Self) -> Result<(), String> {
        if self.times.len() != other.times.len() || self.prices.len() != other.prices.len() {
            return Err(format!(
                "step count mismatch: {} vs {}",
                self.times.len(),
                other.times.len()
            ));
        }
        for (i, (a, b)) in self.times.iter().zip(other.times.iter()).enumerate() {
            if a != b {
                return Err(format!("time mismatch at step {i}"));
            }
        }
        for (i, (a, b)) in self.prices.iter().zip(other.prices.iter()).enumerate() {
            a.bit_eq(b)
                .map_err(|e| format!("price mismatch at step {i}: {e}"))?;
        }
        Ok(())
    }
}

fn fixtures_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/reference")
}

fn parse_i64_col(value: &serde_json::Value, name: &str) -> Result<Vec<i64>, String> {
    let col = Column::from_json(value).map_err(|e| format!("{name}: {e}"))?;
    match col.data {
        ColumnData::Int64(v) => Ok(v),
        _ => Err(format!("{name}: expected int64")),
    }
}

fn parse_f64_col(value: &serde_json::Value, name: &str) -> Result<Vec<f64>, String> {
    let col = Column::from_json(value).map_err(|e| format!("{name}: {e}"))?;
    match col.data {
        ColumnData::Float64(v) => Ok(v),
        _ => Err(format!("{name}: expected float64")),
    }
}

fn bars_from_obj(obj: &serde_json::Value, label: TimeLabel) -> Result<BarColumns, String> {
    let batch = obj.as_object().ok_or("batch must be object")?;
    Ok(BarColumns {
        time: parse_i64_col(batch.get("time").ok_or("missing time")?, "time")?,
        open: parse_f64_col(batch.get("open").ok_or("missing open")?, "open")?,
        high: parse_f64_col(batch.get("high").ok_or("missing high")?, "high")?,
        low: parse_f64_col(batch.get("low").ok_or("missing low")?, "low")?,
        close: parse_f64_col(batch.get("close").ok_or("missing close")?, "close")?,
        tick_volume: None,
        spread: None,
        real_volume: None,
        label,
    })
}

fn label_from_params(params: &serde_json::Value) -> Result<TimeLabel, String> {
    let tz = params
        .get("tz")
        .and_then(|v| v.as_str())
        .ok_or("missing params.tz")?;
    TimeLabel::parse(tz).map_err(|e| e.to_string())
}

fn bound_from_params(params: &serde_json::Value) -> Result<usize, String> {
    params
        .get("bound")
        .and_then(|v| v.as_u64())
        .map(|v| v as usize)
        .ok_or_else(|| "missing params.bound".to_string())
}

fn replay(file: &FixtureFile) -> Result<WindowTrace, String> {
    let vols = VolumeSet {
        tick_volume: false,
        spread: false,
        real_volume: false,
    };
    let mut window: Option<RollingBarWindow> = None;
    let mut times = Vec::new();
    let mut prices = Vec::new();

    for case in &file.cases {
        let params = case.get("params").ok_or("missing params")?;
        let bound = bound_from_params(params)?;
        let label = label_from_params(params)?;
        let operation = case
            .get("operation")
            .and_then(|v| v.as_str())
            .ok_or("missing operation")?;
        let batch = bars_from_obj(case.get("batch").ok_or("missing batch")?, label)?;

        if window.is_none() {
            window = Some(RollingBarWindow::new(bound, vols, label).map_err(|e| e.to_string())?);
        }
        let w = window.as_mut().unwrap();
        match operation {
            "seed_window" | "ingest_completed_bars" => {
                // seed_window is ingest into an empty/new window (plan decision).
                w.ingest_completed(batch).map_err(|e| e.to_string())?;
            }
            other => return Err(format!("unknown operation {other}")),
        }

        let completed = w.completed();
        times.push(completed.time().to_vec());
        let mut step_prices = Vec::with_capacity(completed.len() * 4);
        step_prices.extend_from_slice(completed.open());
        step_prices.extend_from_slice(completed.high());
        step_prices.extend_from_slice(completed.low());
        step_prices.extend_from_slice(completed.close());
        prices.push(step_prices);
    }

    Ok(WindowTrace { times, prices })
}

fn compare_step(
    file: &FixtureFile,
    step_i: usize,
    case: &serde_json::Value,
    actual_times: &[i64],
    actual_prices: &[f64],
) -> Result<(), Box<GateFailure>> {
    let expected = case
        .get("expected")
        .and_then(|v| v.get("window"))
        .ok_or_else(|| {
            Box::new(GateFailure::UnexpectedError {
                function_id: file.fixture_id.clone(),
                case_id: format!("step={step_i}"),
                message: "missing expected.window".to_string(),
            })
        })?;
    let exp_time = parse_i64_col(expected.get("time").unwrap(), "time").map_err(|message| {
        Box::new(GateFailure::UnexpectedError {
            function_id: file.fixture_id.clone(),
            case_id: format!("step={step_i}"),
            message,
        })
    })?;
    if exp_time != actual_times {
        return Err(Box::new(GateFailure::Golden {
            function_id: file.fixture_id.clone(),
            case_id: format!("step={step_i}"),
            mismatch: Mismatch {
                output: "time".to_string(),
                index: 0,
                expected: format!("{exp_time:?}"),
                actual: format!("{actual_times:?}"),
                kind: MismatchKind::Int64,
            },
        }));
    }

    let n = actual_times.len();
    for (name, offset) in [("open", 0), ("high", 1), ("low", 2), ("close", 3)] {
        let exp = parse_f64_col(expected.get(name).unwrap(), name).map_err(|message| {
            Box::new(GateFailure::UnexpectedError {
                function_id: file.fixture_id.clone(),
                case_id: format!("step={step_i}"),
                message,
            })
        })?;
        let act = &actual_prices[offset * n..(offset + 1) * n];
        if let Err((index, kind)) = compare_f64(&file.policy, &exp, act) {
            return Err(Box::new(GateFailure::Golden {
                function_id: file.fixture_id.clone(),
                case_id: format!("step={step_i}"),
                mismatch: Mismatch {
                    output: name.to_string(),
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

#[test]
fn bar_window_reference_gate() {
    let root = fixtures_root();
    let files = load_family(&root, "bar_window").expect("load_family");
    let pending = parse_pending("").expect("parse_pending");
    let mut failures = check_accounting(&files, BOUND_SCENARIOS, &pending, BACKEND_REV.trim());

    for file in &files {
        match replay(file) {
            Ok(trace) => {
                for (step_i, case) in file.cases.iter().enumerate() {
                    if let Err(f) = compare_step(
                        file,
                        step_i,
                        case,
                        &trace.times[step_i],
                        &trace.prices[step_i],
                    ) {
                        failures.push(*f);
                    }
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
fn bar_window_double_run() {
    let root = fixtures_root();
    let files = load_family(&root, "bar_window").expect("load_family");
    for file in &files {
        check_double_run_with(|| replay(file).expect("replay"))
            .unwrap_or_else(|e| panic!("{} nondeterministic: {e}", file.fixture_id));
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
fn bar_window_negative_controls() {
    let root = fixtures_root();
    let files = load_family(&root, "bar_window").expect("load_family");
    let file = files
        .iter()
        .find(|f| f.fixture_id == "s03_redeliver")
        .expect("s03_redeliver");

    // One-ULP close at step 2, index 19.
    {
        let mut case = file.cases[2].clone();
        let close = case
            .pointer_mut("/expected/window/close/values/19")
            .unwrap();
        let bits = close.as_f64().unwrap().to_bits() + 1;
        *close = serde_json::Number::from_f64(f64::from_bits(bits))
            .map(serde_json::Value::Number)
            .unwrap();
        // Keep checksum consistent so compare_f64 sees Bits, not checksum error.
        let values: Vec<f64> = case["expected"]["window"]["close"]["values"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_f64().unwrap())
            .collect();
        let checksum = q_parity::fixture::fnv1a64_float64(&values);
        case["expected"]["window"]["close"]["bits_fnv1a64"] =
            serde_json::Value::String(format!("0x{checksum:016x}"));

        let mut mutated = clone_fixture(file);
        mutated.cases[2] = case;
        let trace = replay(&mutated).unwrap();
        let err = compare_step(
            &mutated,
            2,
            &mutated.cases[2],
            &trace.times[2],
            &trace.prices[2],
        )
        .unwrap_err();
        match *err {
            GateFailure::Golden { mismatch, .. } if mismatch.kind == MismatchKind::Bits => {}
            other => panic!("expected Bits golden failure, got {other:?}"),
        }
    }

    // +1 µs on an expected time.
    {
        let mut case = file.cases[2].clone();
        let t = case.pointer_mut("/expected/window/time/values/19").unwrap();
        *t = serde_json::json!(t.as_i64().unwrap() + 1);
        let values: Vec<i64> = case["expected"]["window"]["time"]["values"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_i64().unwrap())
            .collect();
        let checksum = q_parity::fixture::fnv1a64_int64(&values);
        case["expected"]["window"]["time"]["bits_fnv1a64"] =
            serde_json::Value::String(format!("0x{checksum:016x}"));
        let mut mutated = clone_fixture(file);
        mutated.cases[2] = case;
        let trace = replay(&mutated).unwrap();
        let err = compare_step(
            &mutated,
            2,
            &mutated.cases[2],
            &trace.times[2],
            &trace.prices[2],
        )
        .unwrap_err();
        match *err {
            GateFailure::Golden { mismatch, .. } if mismatch.kind == MismatchKind::Int64 => {}
            other => panic!("expected Int64 golden failure, got {other:?}"),
        }
    }

    // Flipped value bit breaks checksum on from_json.
    {
        let mut case = file.cases[2].clone();
        let close = case
            .pointer_mut("/expected/window/close/values/19")
            .unwrap();
        let bits = close.as_f64().unwrap().to_bits() ^ 1;
        *close = serde_json::Number::from_f64(f64::from_bits(bits))
            .map(serde_json::Value::Number)
            .unwrap();
        // Leave checksum stale.
        let err = Column::from_json(&case["expected"]["window"]["close"]).unwrap_err();
        assert!(err.contains("checksum"), "got {err}");
    }

    // Counter closure fails double-run.
    {
        static CTR: AtomicU64 = AtomicU64::new(0);
        let err = check_double_run_with(|| {
            let mut trace = replay(file).unwrap();
            let n = CTR.fetch_add(1, Ordering::SeqCst);
            if n > 0 {
                let last = trace.prices.last_mut().unwrap();
                last[0] = f64::from_bits(last[0].to_bits() + 1);
            }
            trace
        });
        assert!(err.is_err(), "counter closure should fail double-run");
    }

    // Extra unaccounted file.
    {
        let tmp = tempfile_family_with_extra(&files);
        let loaded = load_family(&tmp, "bar_window").unwrap();
        let pending = parse_pending("").unwrap();
        let failures = check_accounting(&loaded, BOUND_SCENARIOS, &pending, BACKEND_REV.trim());
        assert!(
            failures
                .iter()
                .any(|f| matches!(f, GateFailure::Unaccounted { .. })),
            "expected Unaccounted, got {failures:?}"
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    // Unmodified fixtures still pass.
    bar_window_reference_gate();
}

fn tempfile_family_with_extra(files: &[FixtureFile]) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("q_buffers_bar_window_extra_{}", std::process::id()));
    let fam = dir.join("bar_window");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&fam).unwrap();
    for file in files {
        std::fs::copy(&file.path, fam.join(file.path.file_name().unwrap())).unwrap();
    }
    // tenth file
    let sample = std::fs::read_to_string(&files[0].path).unwrap();
    let mut val: serde_json::Value = serde_json::from_str(&sample).unwrap();
    val["fixture_id"] = serde_json::json!("s10_extra");
    std::fs::write(
        fam.join("s10_extra.json"),
        serde_json::to_string_pretty(&val).unwrap(),
    )
    .unwrap();
    dir
}
