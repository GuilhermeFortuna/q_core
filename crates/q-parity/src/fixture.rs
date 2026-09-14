#![forbid(unsafe_code)]

//! Reference fixture loader and FNV-1a checksum verification.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use crate::compare::Policy;

pub const FORMAT: &str = "q-core-reference-fixture/1";
pub const CANONICAL_NAN_BITS: u64 = 0x7FF8000000000000;
pub const FNV_OFFSET: u64 = 0xcbf29ce484222325;
pub const FNV_PRIME: u64 = 0x100000001b3;

#[derive(Debug, Clone, PartialEq)]
pub enum ColumnData {
    Float64(Vec<f64>),
    Int64(Vec<i64>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Column {
    pub data: ColumnData,
    pub bits_fnv1a64: u64,
}

impl Column {
    pub fn parse_with_expected(value: &serde_json::Value) -> Result<(Column, u64, u64), String> {
        let obj = value
            .as_object()
            .ok_or_else(|| "column must be a JSON object".to_string())?;
        let dtype = obj
            .get("dtype")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "missing dtype".to_string())?;
        let bits_str = obj
            .get("bits_fnv1a64")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "missing bits_fnv1a64".to_string())?;
        let stored_bits = u64::from_str_radix(bits_str.trim_start_matches("0x"), 16)
            .map_err(|e| format!("invalid hex in bits_fnv1a64 '{bits_str}': {e}"))?;
        let raw_values = obj
            .get("values")
            .and_then(|v| v.as_array())
            .ok_or_else(|| "missing values array".to_string())?;

        let (data, computed) = match dtype {
            "float64" => {
                let mut vec = Vec::with_capacity(raw_values.len());
                for item in raw_values {
                    let f = match item {
                        serde_json::Value::Null => f64::NAN,
                        serde_json::Value::String(s) => match s.as_str() {
                            "inf" => f64::INFINITY,
                            "-inf" => f64::NEG_INFINITY,
                            other => {
                                return Err(format!("unexpected string in float64 values: {other}"))
                            }
                        },
                        serde_json::Value::Number(num) => num
                            .as_f64()
                            .ok_or_else(|| format!("cannot convert number {num} to f64"))?,
                        other => {
                            return Err(format!(
                                "unexpected JSON value in float64 column: {other:?}"
                            ))
                        }
                    };
                    vec.push(f);
                }
                let computed = fnv1a64_float64(&vec);
                (ColumnData::Float64(vec), computed)
            }
            "int64" => {
                let mut vec = Vec::with_capacity(raw_values.len());
                for item in raw_values {
                    let i = match item {
                        serde_json::Value::Number(num) => num
                            .as_i64()
                            .ok_or_else(|| format!("cannot convert number {num} to i64"))?,
                        other => {
                            return Err(format!("unexpected JSON value in int64 column: {other:?}"))
                        }
                    };
                    vec.push(i);
                }
                let computed = fnv1a64_int64(&vec);
                (ColumnData::Int64(vec), computed)
            }
            other => return Err(format!("unsupported column dtype: {other}")),
        };

        Ok((
            Column {
                data,
                bits_fnv1a64: stored_bits,
            },
            stored_bits,
            computed,
        ))
    }

    pub fn from_json(value: &serde_json::Value) -> Result<Column, String> {
        let (col, stored, computed) = Self::parse_with_expected(value)?;
        if stored != computed {
            return Err(format!(
                "checksum mismatch: stored 0x{stored:016x} != computed 0x{computed:016x}"
            ));
        }
        Ok(col)
    }
}

fn parse_column(
    path: &Path,
    column_name: &str,
    value: &serde_json::Value,
) -> Result<Column, FixtureError> {
    let (col, stored, computed) =
        Column::parse_with_expected(value).map_err(|msg| FixtureError::Parse {
            path: path.to_path_buf(),
            message: format!("column '{column_name}': {msg}"),
        })?;
    if stored != computed {
        return Err(FixtureError::Checksum {
            path: path.to_path_buf(),
            column: column_name.to_string(),
            stored,
            computed,
        });
    }
    Ok(col)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ParamValue {
    Int(i64),
    Float(f64),
    Str(String),
    Bool(bool),
}

pub type Params = BTreeMap<String, ParamValue>;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ColumnRef {
    pub input_id: String,
    pub column: String,
}

impl ColumnRef {
    pub fn parse(s: &str) -> Option<Self> {
        let (input_id, column) = s.split_once('.')?;
        Some(Self {
            input_id: input_id.to_string(),
            column: column.to_string(),
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Expected {
    Outputs(BTreeMap<String, Column>),
    Rejected { python_exception: String },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Case {
    pub case_id: String,
    pub inputs: BTreeMap<String, ColumnRef>,
    pub params: Params,
    pub expected: Expected,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Provenance {
    pub backend_repo: String,
    pub backend_rev: String,
    pub source_path: String,
    pub source_blob: String,
    pub exporter: String,
    pub numpy: String,
    pub pandas: String,
    pub cpu_level: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FunctionFixture {
    pub function_id: String,
    pub policy: Policy,
    pub provenance: Provenance,
    pub cases: Vec<Case>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct InputSet {
    pub input_id: String,
    pub backend_rev: String,
    pub columns: BTreeMap<String, Column>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ReferenceSet {
    pub inputs: BTreeMap<String, InputSet>,
    pub functions: BTreeMap<String, FunctionFixture>,
}

#[derive(Debug)]
pub enum FixtureError {
    Io {
        path: PathBuf,
        message: String,
    },
    Parse {
        path: PathBuf,
        message: String,
    },
    UnknownFormat {
        path: PathBuf,
        found: String,
    },
    Checksum {
        path: PathBuf,
        column: String,
        stored: u64,
        computed: u64,
    },
    DanglingInput {
        path: PathBuf,
        case_id: String,
        reference: String,
    },
}

impl fmt::Display for FixtureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FixtureError::Io { path, message } => {
                write!(f, "IO error in {}: {}", path.display(), message)
            }
            FixtureError::Parse { path, message } => {
                write!(f, "Parse error in {}: {}", path.display(), message)
            }
            FixtureError::UnknownFormat { path, found } => {
                write!(
                    f,
                    "Unknown format in {}: expected {}, found {}",
                    path.display(),
                    FORMAT,
                    found
                )
            }
            FixtureError::Checksum {
                path,
                column,
                stored,
                computed,
            } => {
                write!(
                    f,
                    "Checksum mismatch in {} for column '{}': stored 0x{:016x}, computed 0x{:016x}",
                    path.display(),
                    column,
                    stored,
                    computed
                )
            }
            FixtureError::DanglingInput {
                path,
                case_id,
                reference,
            } => {
                write!(
                    f,
                    "Dangling input reference in {} (case '{}'): '{}'",
                    path.display(),
                    case_id,
                    reference
                )
            }
        }
    }
}

impl std::error::Error for FixtureError {}

pub struct FixtureFile {
    pub path: PathBuf,
    pub family: String,
    pub fixture_id: String,
    pub policy: Policy,
    pub provenance: Provenance,
    pub cases: Vec<serde_json::Value>,
}

pub fn fnv1a64_float64(values: &[f64]) -> u64 {
    let mut h = FNV_OFFSET;
    for &v in values {
        let raw = if v.is_nan() {
            CANONICAL_NAN_BITS.to_le_bytes()
        } else {
            v.to_le_bytes()
        };
        for b in raw {
            h = (h ^ (b as u64)).wrapping_mul(FNV_PRIME);
        }
    }
    h
}

pub fn fnv1a64_int64(values: &[i64]) -> u64 {
    let mut h = FNV_OFFSET;
    for &v in values {
        let raw = v.to_le_bytes();
        for b in raw {
            h = (h ^ (b as u64)).wrapping_mul(FNV_PRIME);
        }
    }
    h
}

fn parse_provenance(path: &Path, val: &serde_json::Value) -> Result<Provenance, FixtureError> {
    let obj = val.as_object().ok_or_else(|| FixtureError::Parse {
        path: path.to_path_buf(),
        message: "provenance must be an object".to_string(),
    })?;
    let backend_repo = obj
        .get("backend_repo")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let backend_rev = obj
        .get("backend_rev")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let exporter = obj
        .get("exporter")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let numpy = obj
        .get("numpy")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let pandas = obj
        .get("pandas")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let cpu_level = obj
        .get("cpu_level")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let (source_path, source_blob) =
        if let Some(src) = obj.get("source").and_then(|v| v.as_object()) {
            let p = src
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let b = src
                .get("blob")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            (p, b)
        } else if let Some(gen) = obj.get("generator").and_then(|v| v.as_object()) {
            let p = gen
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let b = gen
                .get("blob")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            (p, b)
        } else {
            (String::new(), String::new())
        };

    Ok(Provenance {
        backend_repo,
        backend_rev,
        source_path,
        source_blob,
        exporter,
        numpy,
        pandas,
        cpu_level,
    })
}

pub fn load_family(root: &Path, family: &str) -> Result<Vec<FixtureFile>, FixtureError> {
    let dir = root.join(family);
    let read_dir = std::fs::read_dir(&dir).map_err(|e| FixtureError::Io {
        path: dir.clone(),
        message: e.to_string(),
    })?;

    let mut files = Vec::new();
    for entry in read_dir {
        let entry = entry.map_err(|e| FixtureError::Io {
            path: dir.clone(),
            message: e.to_string(),
        })?;
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }

        let content = std::fs::read_to_string(&path).map_err(|e| FixtureError::Io {
            path: path.clone(),
            message: e.to_string(),
        })?;

        let val: serde_json::Value =
            serde_json::from_str(&content).map_err(|e| FixtureError::Parse {
                path: path.clone(),
                message: e.to_string(),
            })?;

        let format = val.get("format").and_then(|v| v.as_str()).unwrap_or("");
        if format != FORMAT {
            return Err(FixtureError::UnknownFormat {
                path: path.clone(),
                found: format.to_string(),
            });
        }

        let fixture_id = val
            .get("fixture_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| FixtureError::Parse {
                path: path.clone(),
                message: "missing fixture_id".to_string(),
            })?
            .to_string();

        let policy_val = val.get("policy").ok_or_else(|| FixtureError::Parse {
            path: path.clone(),
            message: "missing policy".to_string(),
        })?;
        let policy: Policy =
            serde_json::from_value(policy_val.clone()).map_err(|e| FixtureError::Parse {
                path: path.clone(),
                message: format!("invalid policy: {e}"),
            })?;

        let prov_val = val.get("provenance").ok_or_else(|| FixtureError::Parse {
            path: path.clone(),
            message: "missing provenance".to_string(),
        })?;
        let provenance = parse_provenance(&path, prov_val)?;

        let cases_val =
            val.get("cases")
                .and_then(|v| v.as_array())
                .ok_or_else(|| FixtureError::Parse {
                    path: path.clone(),
                    message: "missing cases array".to_string(),
                })?;

        files.push(FixtureFile {
            path,
            family: family.to_string(),
            fixture_id,
            policy,
            provenance,
            cases: cases_val.clone(),
        });
    }

    files.sort_by(|a, b| a.fixture_id.cmp(&b.fixture_id));
    Ok(files)
}

pub fn load_inputs(root: &Path) -> Result<BTreeMap<String, InputSet>, FixtureError> {
    let dir = root.join("inputs");
    let read_dir = std::fs::read_dir(&dir).map_err(|e| FixtureError::Io {
        path: dir.clone(),
        message: e.to_string(),
    })?;

    let mut map = BTreeMap::new();
    for entry in read_dir {
        let entry = entry.map_err(|e| FixtureError::Io {
            path: dir.clone(),
            message: e.to_string(),
        })?;
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }

        let content = std::fs::read_to_string(&path).map_err(|e| FixtureError::Io {
            path: path.clone(),
            message: e.to_string(),
        })?;

        let val: serde_json::Value =
            serde_json::from_str(&content).map_err(|e| FixtureError::Parse {
                path: path.clone(),
                message: e.to_string(),
            })?;

        let format = val.get("format").and_then(|v| v.as_str()).unwrap_or("");
        if format != FORMAT {
            return Err(FixtureError::UnknownFormat {
                path: path.clone(),
                found: format.to_string(),
            });
        }

        let input_id = val
            .get("input_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| FixtureError::Parse {
                path: path.clone(),
                message: "missing input_id".to_string(),
            })?
            .to_string();

        let backend_rev = val
            .get("provenance")
            .and_then(|p| p.get("backend_rev"))
            .and_then(|r| r.as_str())
            .unwrap_or("")
            .to_string();

        let raw_cols = val
            .get("columns")
            .and_then(|v| v.as_object())
            .ok_or_else(|| FixtureError::Parse {
                path: path.clone(),
                message: "missing columns object".to_string(),
            })?;

        let mut columns = BTreeMap::new();
        for (col_name, col_val) in raw_cols {
            let col = parse_column(&path, col_name, col_val)?;
            columns.insert(col_name.clone(), col);
        }

        map.insert(
            input_id.clone(),
            InputSet {
                input_id,
                backend_rev,
                columns,
            },
        );
    }

    Ok(map)
}

pub fn load_reference_set(root: &Path, family: &str) -> Result<ReferenceSet, FixtureError> {
    let inputs = load_inputs(root)?;
    let fixture_files = load_family(root, family)?;

    let mut functions = BTreeMap::new();
    for file in fixture_files {
        let mut cases = Vec::with_capacity(file.cases.len());
        for case_val in &file.cases {
            let case_obj = case_val.as_object().ok_or_else(|| FixtureError::Parse {
                path: file.path.clone(),
                message: "case must be an object".to_string(),
            })?;

            let case_id = case_obj
                .get("case_id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| FixtureError::Parse {
                    path: file.path.clone(),
                    message: "missing case_id".to_string(),
                })?
                .to_string();

            let inputs_obj = case_obj
                .get("inputs")
                .and_then(|v| v.as_object())
                .ok_or_else(|| FixtureError::Parse {
                    path: file.path.clone(),
                    message: format!("missing inputs in case '{case_id}'"),
                })?;

            let mut case_inputs = BTreeMap::new();
            for (arg_name, ref_val) in inputs_obj {
                let ref_str = ref_val.as_str().ok_or_else(|| FixtureError::Parse {
                    path: file.path.clone(),
                    message: format!("input reference for '{arg_name}' must be a string"),
                })?;
                let col_ref = ColumnRef::parse(ref_str).ok_or_else(|| FixtureError::Parse {
                    path: file.path.clone(),
                    message: format!("invalid input reference '{ref_str}' in case '{case_id}'"),
                })?;

                match inputs.get(&col_ref.input_id) {
                    Some(set) => {
                        if !set.columns.contains_key(&col_ref.column) {
                            return Err(FixtureError::DanglingInput {
                                path: file.path.clone(),
                                case_id: case_id.clone(),
                                reference: ref_str.to_string(),
                            });
                        }
                    }
                    None => {
                        return Err(FixtureError::DanglingInput {
                            path: file.path.clone(),
                            case_id: case_id.clone(),
                            reference: ref_str.to_string(),
                        });
                    }
                }

                case_inputs.insert(arg_name.clone(), col_ref);
            }

            let params_val = case_obj.get("params").ok_or_else(|| FixtureError::Parse {
                path: file.path.clone(),
                message: format!("missing params in case '{case_id}'"),
            })?;
            let params: Params =
                serde_json::from_value(params_val.clone()).map_err(|e| FixtureError::Parse {
                    path: file.path.clone(),
                    message: format!("invalid params in case '{case_id}': {e}"),
                })?;

            let exp_obj = case_obj
                .get("expected")
                .and_then(|v| v.as_object())
                .ok_or_else(|| FixtureError::Parse {
                    path: file.path.clone(),
                    message: format!("missing expected in case '{case_id}'"),
                })?;

            let expected = if let Some(rej_val) = exp_obj.get("rejected") {
                let rej_obj = rej_val.as_object().ok_or_else(|| FixtureError::Parse {
                    path: file.path.clone(),
                    message: format!("rejected must be an object in case '{case_id}'"),
                })?;
                let python_exception = rej_obj
                    .get("python_exception")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                Expected::Rejected { python_exception }
            } else if let Some(out_val) = exp_obj.get("outputs") {
                let out_obj = out_val.as_object().ok_or_else(|| FixtureError::Parse {
                    path: file.path.clone(),
                    message: format!("outputs must be an object in case '{case_id}'"),
                })?;
                let mut outputs = BTreeMap::new();
                for (out_name, col_json) in out_obj {
                    let col = parse_column(&file.path, out_name, col_json)?;
                    outputs.insert(out_name.clone(), col);
                }
                Expected::Outputs(outputs)
            } else {
                return Err(FixtureError::Parse {
                    path: file.path.clone(),
                    message: format!(
                        "expected must contain rejected or outputs in case '{case_id}'"
                    ),
                });
            };

            cases.push(Case {
                case_id,
                inputs: case_inputs,
                params,
                expected,
            });
        }

        functions.insert(
            file.fixture_id.clone(),
            FunctionFixture {
                function_id: file.fixture_id,
                policy: file.policy,
                provenance: file.provenance,
                cases,
            },
        );
    }

    Ok(ReferenceSet { inputs, functions })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_fnv1a64_literals() {
        assert_eq!(fnv1a64_float64(&[]), 0xcbf29ce484222325);
        assert_eq!(
            fnv1a64_float64(&[1.0, -0.0, f64::NAN, f64::INFINITY]),
            0x892eab94389f5cb0
        );

        // NaN with different bit pattern must hash identical to canonical NaN
        let alt_nan = f64::from_bits(0xfff8000000000000);
        assert_eq!(
            fnv1a64_float64(&[1.0, -0.0, alt_nan, f64::INFINITY]),
            0x892eab94389f5cb0
        );
    }

    #[test]
    fn test_load_reference_set_indicators() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/reference");
        let ref_set =
            load_reference_set(&root, "indicators").expect("load_reference_set should succeed");

        assert_eq!(ref_set.inputs.len(), 5);
        assert_eq!(ref_set.functions.len(), 16);

        // Verify known input sets
        assert!(ref_set.inputs.contains_key("synthetic_ohlcv_n400"));
        assert!(ref_set.inputs.contains_key("constant_n60"));
        assert!(ref_set.inputs.contains_key("monotonic_up_n60"));
        assert!(ref_set.inputs.contains_key("nan_gaps_n120"));
        assert!(ref_set.inputs.contains_key("short_n3"));

        // Verify known functions
        assert!(ref_set.functions.contains_key("rsi"));
        assert!(ref_set.functions.contains_key("macd"));
        assert!(ref_set.functions.contains_key("bollinger_bands"));
    }

    #[test]
    fn test_checksum_failure_on_one_ulp_change() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/reference");
        let temp_dir = tempfile_dir("test_ulp");
        copy_dir_all(&root, &temp_dir);

        // Read short_n3.json and modify one float by 1 ULP
        let input_path = temp_dir.join("inputs/short_n3.json");
        let content = fs::read_to_string(&input_path).unwrap();
        let mut json: serde_json::Value = serde_json::from_str(&content).unwrap();
        let val = json["columns"]["close"]["values"][0].as_f64().unwrap();
        let val_ulp = f64::from_bits(val.to_bits() + 1);
        json["columns"]["close"]["values"][0] = serde_json::json!(val_ulp);
        fs::write(&input_path, serde_json::to_string_pretty(&json).unwrap()).unwrap();

        let err = load_reference_set(&temp_dir, "indicators").unwrap_err();
        match err {
            FixtureError::Checksum { column, .. } => {
                assert_eq!(column, "close");
            }
            other => panic!("expected Checksum error, got: {other:?}"),
        }
        let _ = fs::remove_dir_all(temp_dir);
    }

    #[test]
    fn test_unknown_format_failure() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/reference");
        let temp_dir = tempfile_dir("test_format");
        copy_dir_all(&root, &temp_dir);

        // Modify format in rsi.json
        let rsi_path = temp_dir.join("indicators/rsi.json");
        let content = fs::read_to_string(&rsi_path).unwrap();
        let mut json: serde_json::Value = serde_json::from_str(&content).unwrap();
        json["format"] = serde_json::json!("q-core-reference-fixture/2");
        fs::write(&rsi_path, serde_json::to_string_pretty(&json).unwrap()).unwrap();

        let err = load_reference_set(&temp_dir, "indicators").unwrap_err();
        match err {
            FixtureError::UnknownFormat { found, .. } => {
                assert_eq!(found, "q-core-reference-fixture/2");
            }
            other => panic!("expected UnknownFormat error, got: {other:?}"),
        }
        let _ = fs::remove_dir_all(temp_dir);
    }

    #[test]
    fn test_dangling_input_failure() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/reference");
        let temp_dir = tempfile_dir("test_dangling");
        copy_dir_all(&root, &temp_dir);

        // Delete short_n3.json so cases referencing short_n3 become dangling
        let input_path = temp_dir.join("inputs/short_n3.json");
        fs::remove_file(input_path).unwrap();

        let err = load_reference_set(&temp_dir, "indicators").unwrap_err();
        match err {
            FixtureError::DanglingInput { reference, .. } => {
                assert!(reference.starts_with("short_n3."));
            }
            other => panic!("expected DanglingInput error, got: {other:?}"),
        }
        let _ = fs::remove_dir_all(temp_dir);
    }

    fn tempfile_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("q_parity_{}_{}", name, std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn copy_dir_all(src: &Path, dst: &Path) {
        fs::create_dir_all(dst).unwrap();
        for entry in fs::read_dir(src).unwrap() {
            let entry = entry.unwrap();
            let ty = entry.file_type().unwrap();
            if ty.is_dir() {
                copy_dir_all(&entry.path(), &dst.join(entry.file_name()));
            } else {
                fs::copy(entry.path(), dst.join(entry.file_name())).unwrap();
            }
        }
    }
}
