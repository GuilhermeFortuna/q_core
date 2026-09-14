#![forbid(unsafe_code)]

//! Numeric comparison policies for the reference parity gate.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;

use crate::fixture::{Column, ColumnData, CANONICAL_NAN_BITS};
use crate::gate::KernelOutputs;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Policy {
    AbsRelTol { abs: f64, rel: f64 },
    Exact,
}

#[derive(Debug, Clone, PartialEq)]
pub enum MismatchKind {
    Length { expected: usize, actual: usize },
    MissingOutput,
    UnexpectedOutput,
    NanPlacement,
    Infinity,
    Magnitude { abs_diff: f64 },
    Bits,
    Int64,
    // Aliases
    NanMismatch,
    InfSignMismatch,
    ZeroSignMismatch,
    LengthMismatch,
}

impl fmt::Display for MismatchKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MismatchKind::Length { expected, actual } => {
                write!(f, "length mismatch: expected {expected}, actual {actual}")
            }
            MismatchKind::MissingOutput => write!(f, "missing expected output"),
            MismatchKind::UnexpectedOutput => write!(f, "unexpected output"),
            MismatchKind::NanPlacement | MismatchKind::NanMismatch => {
                write!(f, "NaN placement mismatch")
            }
            MismatchKind::Infinity | MismatchKind::InfSignMismatch => {
                write!(f, "infinity sign mismatch")
            }
            MismatchKind::Magnitude { abs_diff } => {
                write!(f, "magnitude difference {abs_diff} exceeds tolerance")
            }
            MismatchKind::Bits | MismatchKind::ZeroSignMismatch => {
                write!(f, "exact bit difference")
            }
            MismatchKind::Int64 => write!(f, "int64 value mismatch"),
            MismatchKind::LengthMismatch => write!(f, "length mismatch"),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Mismatch {
    pub output: String,
    pub index: usize,
    pub expected: String,
    pub actual: String,
    pub kind: MismatchKind,
}

impl fmt::Display for Mismatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "output '{}' mismatch at index {}: expected {}, actual {} ({})",
            self.output, self.index, self.expected, self.actual, self.kind
        )
    }
}

pub fn compare_f64(
    policy: &Policy,
    expected: &[f64],
    actual: &[f64],
) -> Result<(), (usize, MismatchKind)> {
    if expected.len() != actual.len() {
        return Err((
            0,
            MismatchKind::Length {
                expected: expected.len(),
                actual: actual.len(),
            },
        ));
    }

    match policy {
        Policy::AbsRelTol { abs, rel } => {
            for (i, (&e, &a)) in expected.iter().zip(actual.iter()).enumerate() {
                if e.is_nan() && a.is_nan() {
                    continue;
                }
                if e.is_nan() || a.is_nan() {
                    return Err((i, MismatchKind::NanPlacement));
                }
                if e.is_infinite() || a.is_infinite() {
                    if e.is_infinite()
                        && a.is_infinite()
                        && (e.is_sign_positive() == a.is_sign_positive())
                    {
                        continue;
                    }
                    return Err((i, MismatchKind::Infinity));
                }

                // Finite comparison: -0.0 vs 0.0 naturally has diff 0.0 <= abs
                let diff = (a - e).abs();
                if diff <= *abs || diff <= *rel * e.abs() {
                    continue;
                }
                return Err((i, MismatchKind::Magnitude { abs_diff: diff }));
            }
        }
        Policy::Exact => {
            for (i, (&e, &a)) in expected.iter().zip(actual.iter()).enumerate() {
                let e_bits = if e.is_nan() {
                    CANONICAL_NAN_BITS
                } else {
                    e.to_bits()
                };
                let a_bits = if a.is_nan() {
                    CANONICAL_NAN_BITS
                } else {
                    a.to_bits()
                };
                if e_bits != a_bits {
                    return Err((i, MismatchKind::Bits));
                }
            }
        }
    }

    Ok(())
}

pub fn compare_outputs(
    policy: &Policy,
    expected: &BTreeMap<String, Column>,
    actual: &KernelOutputs,
) -> Result<(), Mismatch> {
    for (name, exp_col) in expected {
        let act_vec = match actual.0.get(name) {
            Some(v) => v,
            None => {
                return Err(Mismatch {
                    output: name.clone(),
                    index: 0,
                    expected: "present".to_string(),
                    actual: "missing".to_string(),
                    kind: MismatchKind::MissingOutput,
                });
            }
        };

        match &exp_col.data {
            ColumnData::Float64(exp_vec) => {
                if let Err((idx, kind)) = compare_f64(policy, exp_vec, act_vec) {
                    let exp_str = exp_vec.get(idx).map(|v| v.to_string()).unwrap_or_default();
                    let act_str = act_vec.get(idx).map(|v| v.to_string()).unwrap_or_default();
                    return Err(Mismatch {
                        output: name.clone(),
                        index: idx,
                        expected: exp_str,
                        actual: act_str,
                        kind,
                    });
                }
            }
            ColumnData::Int64(exp_vec) => {
                if exp_vec.len() != act_vec.len() {
                    return Err(Mismatch {
                        output: name.clone(),
                        index: 0,
                        expected: format!("len {}", exp_vec.len()),
                        actual: format!("len {}", act_vec.len()),
                        kind: MismatchKind::Length {
                            expected: exp_vec.len(),
                            actual: act_vec.len(),
                        },
                    });
                }
                for (i, (&e, &a)) in exp_vec.iter().zip(act_vec.iter()).enumerate() {
                    if e != a as i64 {
                        return Err(Mismatch {
                            output: name.clone(),
                            index: i,
                            expected: e.to_string(),
                            actual: a.to_string(),
                            kind: MismatchKind::Int64,
                        });
                    }
                }
            }
        }
    }

    for act_name in actual.0.keys() {
        if !expected.contains_key(act_name) {
            return Err(Mismatch {
                output: act_name.clone(),
                index: 0,
                expected: "none".to_string(),
                actual: "present".to_string(),
                kind: MismatchKind::UnexpectedOutput,
            });
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixture::{load_reference_set, Expected};
    use std::path::Path;

    #[test]
    fn test_compare_f64_abs_rel_tol() {
        let policy = Policy::AbsRelTol {
            abs: 1e-10,
            rel: 1e-12,
        };

        // [1.0] against [1.0 + 5e-11] passes through the absolute bound
        assert!(compare_f64(&policy, &[1.0], &[1.0 + 5e-11]).is_ok());

        // [1.0] against [1.0 + 2e-10] exceeds both bounds and fails with Magnitude at index 0
        let err = compare_f64(&policy, &[1.0], &[1.0 + 2e-10]).unwrap_err();
        assert_eq!(err.0, 0);
        assert!(matches!(err.1, MismatchKind::Magnitude { .. }));

        // [130000.0] against [130000.0 + 1e-8] is above absolute bound (1e-10)
        // but within relative bound (1.3e-7), so it passes through relative bound
        assert!(compare_f64(&policy, &[130000.0], &[130000.0 + 1e-8]).is_ok());

        // [130000.0] against [130000.0 + 2e-7] exceeds both bounds and fails with Magnitude
        let err = compare_f64(&policy, &[130000.0], &[130000.0 + 2e-7]).unwrap_err();
        assert_eq!(err.0, 0);
        assert!(matches!(err.1, MismatchKind::Magnitude { .. }));

        // NaN at the same index passes
        assert!(compare_f64(&policy, &[f64::NAN], &[f64::NAN]).is_ok());

        // NaN at different indices fails with NanPlacement / NanMismatch
        let err = compare_f64(&policy, &[f64::NAN], &[1.0]).unwrap_err();
        assert_eq!(err.0, 0);
        assert!(matches!(
            err.1,
            MismatchKind::NanPlacement | MismatchKind::NanMismatch
        ));

        let err = compare_f64(&policy, &[1.0], &[f64::NAN]).unwrap_err();
        assert_eq!(err.0, 0);
        assert!(matches!(
            err.1,
            MismatchKind::NanPlacement | MismatchKind::NanMismatch
        ));

        // +inf vs +inf passes
        assert!(compare_f64(&policy, &[f64::INFINITY], &[f64::INFINITY]).is_ok());

        // +inf vs -inf fails with Infinity / InfSignMismatch
        let err = compare_f64(&policy, &[f64::INFINITY], &[f64::NEG_INFINITY]).unwrap_err();
        assert_eq!(err.0, 0);
        assert!(matches!(
            err.1,
            MismatchKind::Infinity | MismatchKind::InfSignMismatch
        ));

        // -0.0 vs 0.0 passes
        assert!(compare_f64(&policy, &[-0.0], &[0.0]).is_ok());
    }

    #[test]
    fn test_compare_f64_exact() {
        let policy = Policy::Exact;

        // Under Exact, -0.0 vs 0.0 fails with Bits / ZeroSignMismatch
        let err = compare_f64(&policy, &[-0.0], &[0.0]).unwrap_err();
        assert_eq!(err.0, 0);
        assert!(matches!(
            err.1,
            MismatchKind::Bits | MismatchKind::ZeroSignMismatch
        ));

        // Same exact bits pass
        assert!(compare_f64(&policy, &[1.0, -0.0], &[1.0, -0.0]).is_ok());

        // NaNs canonicalize and pass under Exact
        let nan1 = f64::NAN;
        let nan2 = f64::from_bits(0xfff8000000000000);
        assert!(compare_f64(&policy, &[nan1], &[nan2]).is_ok());
    }

    #[test]
    fn test_compare_f64_length_mismatch() {
        let policy = Policy::AbsRelTol {
            abs: 1e-10,
            rel: 1e-12,
        };
        let err = compare_f64(&policy, &[1.0], &[1.0, 2.0]).unwrap_err();
        assert_eq!(err.0, 0);
        assert!(matches!(
            err.1,
            MismatchKind::Length {
                expected: 1,
                actual: 2
            } | MismatchKind::LengthMismatch
        ));
    }

    #[test]
    fn test_indicators_reference_round_trip() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/reference");
        let ref_set = load_reference_set(&root, "indicators").expect("load_reference_set failed");

        let mut total_cases = 0;
        let mut computed_cases = 0;
        let mut rejected_cases = 0;

        for fixture in ref_set.functions.values() {
            for case in &fixture.cases {
                total_cases += 1;
                match &case.expected {
                    Expected::Rejected { .. } => {
                        rejected_cases += 1;
                    }
                    Expected::Outputs(outputs) => {
                        computed_cases += 1;
                        let mut kernel_outputs = KernelOutputs::new();
                        for (name, col) in outputs {
                            match &col.data {
                                ColumnData::Float64(vec) => {
                                    kernel_outputs.insert(name.clone(), vec.clone());
                                }
                                ColumnData::Int64(vec) => {
                                    kernel_outputs.insert(
                                        name.clone(),
                                        vec.iter().map(|&x| x as f64).collect(),
                                    );
                                }
                            }
                        }
                        compare_outputs(&fixture.policy, outputs, &kernel_outputs).unwrap_or_else(
                            |e| {
                                panic!(
                                    "round-trip failed in {} case {}: {e}",
                                    fixture.function_id, case.case_id
                                )
                            },
                        );
                    }
                }
            }
        }

        assert_eq!(total_cases, 410);
        assert_eq!(computed_cases, 355);
        assert_eq!(rejected_cases, 55);
    }
}
