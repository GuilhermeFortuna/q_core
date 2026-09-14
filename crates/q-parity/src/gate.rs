#![forbid(unsafe_code)]

//! Reference parity gate definitions and accounting.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use crate::compare::Mismatch;
use crate::fixture::{FixtureFile, Params, ReferenceSet};

#[derive(Debug, Clone)]
pub struct KernelError {
    pub message: String,
}

impl fmt::Display for KernelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for KernelError {}

pub struct KernelInputs<'a> {
    columns: BTreeMap<String, &'a [f64]>,
    len: usize,
}

impl<'a> KernelInputs<'a> {
    pub fn new(columns: BTreeMap<String, &'a [f64]>) -> Self {
        let len = columns.values().next().map(|s| s.len()).unwrap_or(0);
        Self { columns, len }
    }

    pub fn column(&self, name: &str) -> Result<&'a [f64], KernelError> {
        self.columns.get(name).copied().ok_or_else(|| KernelError {
            message: format!("unknown input column '{name}'"),
        })
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn prefix(&self, len: usize) -> KernelInputs<'a> {
        let prefix_len = len.min(self.len);
        let mut cols = BTreeMap::new();
        for (k, v) in &self.columns {
            cols.insert(k.clone(), &v[..prefix_len]);
        }
        KernelInputs {
            columns: cols,
            len: prefix_len,
        }
    }
}

pub type KernelFn = fn(&KernelInputs<'_>, &Params) -> Result<KernelOutputs, KernelError>;

pub struct Binding {
    pub function_id: &'static str,
    pub kernel: KernelFn,
}

#[derive(Debug, Clone, PartialEq)]
pub struct KernelOutputs(pub BTreeMap<String, Vec<f64>>);

impl KernelOutputs {
    pub fn new() -> Self {
        Self(BTreeMap::new())
    }

    pub fn insert(&mut self, name: impl Into<String>, values: Vec<f64>) {
        self.0.insert(name.into(), values);
    }
}

impl Default for KernelOutputs {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pending(pub BTreeSet<String>);

impl Pending {
    pub fn new() -> Self {
        Self(BTreeSet::new())
    }

    pub fn contains(&self, id: &str) -> bool {
        self.0.contains(id)
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &String> {
        self.0.iter()
    }
}

impl Default for Pending {
    fn default() -> Self {
        Self::new()
    }
}

pub fn parse_pending(text: &str) -> Result<Pending, String> {
    let mut set = BTreeSet::new();
    for (line_no, line) in text.lines().enumerate() {
        let trimmed = line.split('#').next().unwrap_or("").trim();
        if trimmed.is_empty() {
            continue;
        }
        if !set.insert(trimmed.to_string()) {
            return Err(format!(
                "duplicate pending id '{trimmed}' on line {}",
                line_no + 1
            ));
        }
    }
    Ok(Pending(set))
}

#[derive(Debug, Clone, PartialEq)]
pub enum GateFailure {
    Unaccounted {
        function_id: String,
    },
    PendingButBound {
        function_id: String,
    },
    PendingUnknown {
        function_id: String,
    },
    BoundUnknown {
        function_id: String,
    },
    DuplicateBinding {
        function_id: String,
    },
    ProvenanceRev {
        file: String,
        recorded: String,
        pinned: String,
    },
    Golden {
        function_id: String,
        case_id: String,
        mismatch: Mismatch,
    },
    AcceptedRejectedCase {
        function_id: String,
        case_id: String,
        python_exception: String,
    },
    UnexpectedError {
        function_id: String,
        case_id: String,
        message: String,
    },
    Nondeterministic {
        function_id: String,
        case_id: String,
        output: String,
        index: usize,
    },
    NonCausal {
        function_id: String,
        case_id: String,
        output: String,
        index: usize,
        full: String,
        prefix: String,
    },
}

impl fmt::Display for GateFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GateFailure::Unaccounted { function_id } => {
                write!(
                    f,
                    "unaccounted fixture: '{function_id}' is neither bound nor pending"
                )
            }
            GateFailure::PendingButBound { function_id } => {
                write!(
                    f,
                    "conflicting state: '{function_id}' is both bound and pending"
                )
            }
            GateFailure::PendingUnknown { function_id } => {
                write!(
                    f,
                    "unknown pending entry: '{function_id}' does not exist in fixtures"
                )
            }
            GateFailure::BoundUnknown { function_id } => {
                write!(
                    f,
                    "unknown bound kernel: '{function_id}' does not exist in fixtures"
                )
            }
            GateFailure::DuplicateBinding { function_id } => {
                write!(f, "duplicate binding for function: '{function_id}'")
            }
            GateFailure::ProvenanceRev {
                file,
                recorded,
                pinned,
            } => {
                write!(f, "provenance backend_rev mismatch in '{file}': recorded {recorded}, pinned {pinned}")
            }
            GateFailure::Golden {
                function_id,
                case_id,
                mismatch,
            } => {
                write!(
                    f,
                    "golden mismatch in {function_id} case '{case_id}': {mismatch}"
                )
            }
            GateFailure::AcceptedRejectedCase {
                function_id,
                case_id,
                python_exception,
            } => {
                write!(f, "rejected case accepted in {function_id} case '{case_id}': expected {python_exception}")
            }
            GateFailure::UnexpectedError {
                function_id,
                case_id,
                message,
            } => {
                write!(
                    f,
                    "unexpected kernel error in {function_id} case '{case_id}': {message}"
                )
            }
            GateFailure::Nondeterministic {
                function_id,
                case_id,
                output,
                index,
            } => {
                write!(f, "nondeterministic output in {function_id} case '{case_id}', output '{output}' at index {index}")
            }
            GateFailure::NonCausal {
                function_id,
                case_id,
                output,
                index,
                full,
                prefix,
            } => {
                write!(f, "non-causal output in {function_id} case '{case_id}', output '{output}' at index {index}: full {full}, prefix {prefix}")
            }
        }
    }
}

pub fn check_accounting(
    files: &[FixtureFile],
    bound_ids: &[&str],
    pending: &Pending,
    pinned_backend_rev: &str,
) -> Vec<GateFailure> {
    let mut failures = Vec::new();
    let file_id_set: BTreeSet<&str> = files.iter().map(|f| f.fixture_id.as_str()).collect();

    // Check duplicate bindings
    let mut seen_bindings = BTreeSet::new();
    for &b_id in bound_ids {
        if !seen_bindings.insert(b_id) {
            failures.push(GateFailure::DuplicateBinding {
                function_id: b_id.to_string(),
            });
        }
    }

    // Check BoundUnknown
    for &b_id in &seen_bindings {
        if !file_id_set.contains(b_id) {
            failures.push(GateFailure::BoundUnknown {
                function_id: b_id.to_string(),
            });
        }
    }

    // Check PendingUnknown
    for p_id in pending.iter() {
        if !file_id_set.contains(p_id.as_str()) {
            failures.push(GateFailure::PendingUnknown {
                function_id: p_id.clone(),
            });
        }
    }

    // Check PendingButBound
    for &b_id in &seen_bindings {
        if pending.contains(b_id) {
            failures.push(GateFailure::PendingButBound {
                function_id: b_id.to_string(),
            });
        }
    }

    // Check Unaccounted
    for file in files {
        let fid = file.fixture_id.as_str();
        if !seen_bindings.contains(fid) && !pending.contains(fid) {
            failures.push(GateFailure::Unaccounted {
                function_id: fid.to_string(),
            });
        }
    }

    // Check ProvenanceRev
    for file in files {
        if file.provenance.backend_rev != pinned_backend_rev {
            failures.push(GateFailure::ProvenanceRev {
                file: file.fixture_id.clone(),
                recorded: file.provenance.backend_rev.clone(),
                pinned: pinned_backend_rev.to_string(),
            });
        }
    }

    failures
}

#[derive(Debug, Clone)]
pub struct GateReport {
    pub bound: Vec<String>,
    pub pending: Vec<String>,
    pub failures: Vec<GateFailure>,
}

impl GateReport {
    pub fn summary(&self) -> String {
        let mut s = format!(
            "reference gate: {} bound, {} pending, {} failures",
            self.bound.len(),
            self.pending.len(),
            self.failures.len()
        );
        for f in &self.failures {
            s.push_str(&format!("\n  failure: {f}"));
        }
        s
    }

    pub fn assert_passed(&self) {
        if !self.failures.is_empty() {
            panic!("{}", self.summary());
        }
    }
}

pub fn run_gate(
    reference: &ReferenceSet,
    bindings: &[Binding],
    pending: &Pending,
    pinned_backend_rev: &str,
) -> GateReport {
    let mut bound_names = Vec::new();
    let mut bound_ids = Vec::new();
    let mut binding_map: BTreeMap<&str, KernelFn> = BTreeMap::new();

    for b in bindings {
        bound_names.push(b.function_id.to_string());
        bound_ids.push(b.function_id);
        binding_map.insert(b.function_id, b.kernel);
    }

    let fixture_files: Vec<FixtureFile> = reference
        .functions
        .values()
        .map(|f| FixtureFile {
            path: std::path::PathBuf::new(),
            family: "indicators".to_string(),
            fixture_id: f.function_id.clone(),
            policy: f.policy.clone(),
            provenance: f.provenance.clone(),
            cases: Vec::new(),
        })
        .collect();

    let mut failures = check_accounting(&fixture_files, &bound_ids, pending, pinned_backend_rev);

    // Evaluate bound kernels
    for (&fid, &kernel) in &binding_map {
        let fixture = match reference.functions.get(fid) {
            Some(f) => f,
            None => continue, // Already handled in check_accounting
        };

        for case in &fixture.cases {
            // Build KernelInputs from case.inputs and reference.inputs
            let mut col_slices: BTreeMap<String, &[f64]> = BTreeMap::new();
            for (arg_name, col_ref) in &case.inputs {
                let input_set = match reference.inputs.get(&col_ref.input_id) {
                    Some(s) => s,
                    None => continue,
                };
                let col = match input_set.columns.get(&col_ref.column) {
                    Some(c) => c,
                    None => continue,
                };
                match &col.data {
                    crate::fixture::ColumnData::Float64(vec) => {
                        col_slices.insert(arg_name.clone(), &vec[..]);
                    }
                    crate::fixture::ColumnData::Int64(_) => continue,
                }
            }

            let kernel_inputs = KernelInputs::new(col_slices);

            match &case.expected {
                crate::fixture::Expected::Rejected { python_exception } => {
                    // Kernel should reject (return Err)
                    if let Ok(_out) = kernel(&kernel_inputs, &case.params) {
                        failures.push(GateFailure::AcceptedRejectedCase {
                            function_id: fid.to_string(),
                            case_id: case.case_id.clone(),
                            python_exception: python_exception.clone(),
                        });
                    }
                }
                crate::fixture::Expected::Outputs(outputs) => {
                    // Double-run check
                    if let Err((out_name, idx)) =
                        crate::determinism::check_double_run(kernel, &kernel_inputs, &case.params)
                    {
                        failures.push(GateFailure::Nondeterministic {
                            function_id: fid.to_string(),
                            case_id: case.case_id.clone(),
                            output: out_name,
                            index: idx,
                        });
                        continue;
                    }

                    // Prefix causality check
                    if let Err(non_causal) = crate::causality::check_prefix_causal(
                        kernel,
                        &kernel_inputs,
                        &case.params,
                        &fixture.policy,
                    ) {
                        failures.push(GateFailure::NonCausal {
                            function_id: fid.to_string(),
                            case_id: case.case_id.clone(),
                            output: non_causal.output,
                            index: non_causal.index,
                            full: non_causal.full.to_string(),
                            prefix: non_causal.prefix.to_string(),
                        });
                        continue;
                    }

                    // Golden output comparison
                    match kernel(&kernel_inputs, &case.params) {
                        Ok(actual) => {
                            if let Err(mismatch) =
                                crate::compare::compare_outputs(&fixture.policy, outputs, &actual)
                            {
                                failures.push(GateFailure::Golden {
                                    function_id: fid.to_string(),
                                    case_id: case.case_id.clone(),
                                    mismatch,
                                });
                            }
                        }
                        Err(e) => {
                            failures.push(GateFailure::UnexpectedError {
                                function_id: fid.to_string(),
                                case_id: case.case_id.clone(),
                                message: e.message,
                            });
                        }
                    }
                }
            }
        }
    }

    let pending_list: Vec<String> = pending.iter().cloned().collect();

    GateReport {
        bound: bound_names,
        pending: pending_list,
        failures,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compare::Policy;
    use crate::determinism::check_double_run_with;
    use crate::fixture::{fnv1a64_float64, fnv1a64_int64, load_family, Column, ColumnData};
    use std::fs;
    use std::path::PathBuf;

    #[test]
    fn test_family_mechanism_with_test_scenario() {
        let temp_dir = tempfile_dir("family_mech");
        let scenario_dir = temp_dir.join("test_scenario");
        fs::create_dir_all(&scenario_dir).unwrap();

        let times = vec![1672617600000000000i64, 1672621200000000000i64];
        let times_h = fnv1a64_int64(&times);
        let close = vec![100.5, 101.2];
        let close_h = fnv1a64_float64(&close);

        let fixture_json = serde_json::json!({
            "format": "q-core-reference-fixture/1",
            "family": "test_scenario",
            "fixture_id": "scenario_1",
            "policy": {
                "kind": "exact"
            },
            "provenance": {
                "backend_repo": "https://github.com/GuilhermeFortuna/q_backend.git",
                "backend_rev": "test_pinned_rev",
                "exporter": "tools/reference/export_reference.py",
                "numpy": "2.4.4",
                "pandas": "3.0.2",
                "cpu_level": "x86-64-v3"
            },
            "cases": [
                {
                    "steps": [
                        {
                            "op": "append",
                            "times": {
                                "dtype": "int64",
                                "bits_fnv1a64": format!("0x{:016x}", times_h),
                                "values": times
                            },
                            "close": {
                                "dtype": "float64",
                                "bits_fnv1a64": format!("0x{:016x}", close_h),
                                "values": close
                            }
                        }
                    ],
                    "expected_after_each": []
                }
            ]
        });

        fs::write(
            scenario_dir.join("scenario_1.json"),
            serde_json::to_string_pretty(&fixture_json).unwrap(),
        )
        .unwrap();

        let files = load_family(&temp_dir, "test_scenario").expect("load_family should succeed");
        assert_eq!(files.len(), 1);
        let file = &files[0];
        assert_eq!(file.fixture_id, "scenario_1");
        assert_eq!(file.policy, Policy::Exact);

        // Verify Column::from_json decodes both columns with verified checksums
        let step = &file.cases[0]["steps"][0];
        let col_times = Column::from_json(&step["times"]).expect("times column should decode");
        match col_times.data {
            ColumnData::Int64(v) => assert_eq!(v, times),
            _ => panic!("expected int64"),
        }
        assert_eq!(col_times.bits_fnv1a64, times_h);

        let col_close = Column::from_json(&step["close"]).expect("close column should decode");
        match col_close.data {
            ColumnData::Float64(v) => assert_eq!(v, close),
            _ => panic!("expected float64"),
        }
        assert_eq!(col_close.bits_fnv1a64, close_h);

        // check_accounting with the id neither bound nor pending gives Unaccounted
        let failures = check_accounting(&files, &[], &Pending::new(), "test_pinned_rev");
        assert_eq!(failures.len(), 1);
        assert_eq!(
            failures[0],
            GateFailure::Unaccounted {
                function_id: "scenario_1".to_string()
            }
        );

        let _ = fs::remove_dir_all(temp_dir);
    }

    #[test]
    fn test_check_double_run_with_fixed_and_counter() {
        use std::sync::atomic::{AtomicI64, Ordering};

        // check_double_run_with passes a closure that returns a fixed Vec<i64>
        assert!(check_double_run_with(|| vec![10i64, 20, 30]).is_ok());

        // fails a closure over the call counter
        static CTR: AtomicI64 = AtomicI64::new(0);
        let err = check_double_run_with(|| vec![CTR.fetch_add(1, Ordering::SeqCst)]);
        assert!(err.is_err());
    }

    fn tempfile_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("q_parity_{}_{}", name, std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }
}
