#![forbid(unsafe_code)]

//! Double-run determinism verification.

use crate::fixture::{Params, CANONICAL_NAN_BITS};
use crate::gate::{KernelFn, KernelInputs, KernelOutputs};

pub trait BitEq {
    fn bit_eq(&self, other: &Self) -> Result<(), String>;
}

impl BitEq for Vec<f64> {
    fn bit_eq(&self, other: &Self) -> Result<(), String> {
        if self.len() != other.len() {
            return Err(format!(
                "length mismatch: {} != {}",
                self.len(),
                other.len()
            ));
        }
        for (i, (&a, &b)) in self.iter().zip(other.iter()).enumerate() {
            let a_bits = if a.is_nan() {
                CANONICAL_NAN_BITS
            } else {
                a.to_bits()
            };
            let b_bits = if b.is_nan() {
                CANONICAL_NAN_BITS
            } else {
                b.to_bits()
            };
            if a_bits != b_bits {
                return Err(format!(
                    "bit mismatch at index {i}: 0x{a_bits:016x} != 0x{b_bits:016x}"
                ));
            }
        }
        Ok(())
    }
}

impl BitEq for Vec<i64> {
    fn bit_eq(&self, other: &Self) -> Result<(), String> {
        if self.len() != other.len() {
            return Err(format!(
                "length mismatch: {} != {}",
                self.len(),
                other.len()
            ));
        }
        for (i, (&a, &b)) in self.iter().zip(other.iter()).enumerate() {
            if a != b {
                return Err(format!("mismatch at index {i}: {a} != {b}"));
            }
        }
        Ok(())
    }
}

impl BitEq for KernelOutputs {
    fn bit_eq(&self, other: &Self) -> Result<(), String> {
        if self.0.len() != other.0.len() {
            return Err(format!(
                "output count mismatch: {} != {}",
                self.0.len(),
                other.0.len()
            ));
        }
        for (name, vec_a) in &self.0 {
            let vec_b = other
                .0
                .get(name)
                .ok_or_else(|| format!("missing output '{name}'"))?;
            vec_a
                .bit_eq(vec_b)
                .map_err(|e| format!("in output '{name}': {e}"))?;
        }
        for name in other.0.keys() {
            if !self.0.contains_key(name) {
                return Err(format!("unexpected output '{name}'"));
            }
        }
        Ok(())
    }
}

pub fn check_double_run_with<T, F>(run: F) -> Result<(), String>
where
    F: Fn() -> T,
    T: BitEq,
{
    let first = run();
    let second = run();
    first.bit_eq(&second)
}

pub fn check_double_run(
    kernel: KernelFn,
    inputs: &KernelInputs<'_>,
    params: &Params,
) -> Result<(), (String, usize)> {
    let out1 = kernel(inputs, params).map_err(|_| ("kernel_error".to_string(), 0))?;
    let out2 = kernel(inputs, params).map_err(|_| ("kernel_error".to_string(), 0))?;

    for (name, vec1) in &out1.0 {
        let vec2 = match out2.0.get(name) {
            Some(v) => v,
            None => return Err((name.clone(), 0)),
        };
        if vec1.len() != vec2.len() {
            return Err((name.clone(), 0));
        }
        for (i, (&v1, &v2)) in vec1.iter().zip(vec2.iter()).enumerate() {
            let b1 = if v1.is_nan() {
                CANONICAL_NAN_BITS
            } else {
                v1.to_bits()
            };
            let b2 = if v2.is_nan() {
                CANONICAL_NAN_BITS
            } else {
                v2.to_bits()
            };
            if b1 != b2 {
                return Err((name.clone(), i));
            }
        }
    }

    for name in out2.0.keys() {
        if !out1.0.contains_key(name) {
            return Err((name.clone(), 0));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gate::KernelError;
    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn identity_kernel(
        inputs: &KernelInputs<'_>,
        _params: &Params,
    ) -> Result<KernelOutputs, KernelError> {
        let col = inputs.column("in")?;
        let mut out = KernelOutputs::new();
        out.insert("out", col.to_vec());
        Ok(out)
    }

    static CALL_COUNT: AtomicU64 = AtomicU64::new(0);

    fn counter_kernel(
        inputs: &KernelInputs<'_>,
        _params: &Params,
    ) -> Result<KernelOutputs, KernelError> {
        let col = inputs.column("in")?;
        let count = CALL_COUNT.fetch_add(1, Ordering::SeqCst) as f64;
        let mut out_vec = col.to_vec();
        if !out_vec.is_empty() {
            out_vec[0] += count;
        }
        let mut out = KernelOutputs::new();
        out.insert("out", out_vec);
        Ok(out)
    }

    #[test]
    fn test_identity_kernel_passes_double_run() {
        let data = [1.0, 2.0, 3.0];
        let mut cols = BTreeMap::new();
        cols.insert("in".to_string(), &data[..]);
        let inputs = KernelInputs::new(cols);
        let params = Params::new();

        assert!(check_double_run(identity_kernel, &inputs, &params).is_ok());
    }

    #[test]
    fn test_counter_kernel_fails_double_run() {
        CALL_COUNT.store(0, Ordering::SeqCst);
        let data = [1.0, 2.0, 3.0];
        let mut cols = BTreeMap::new();
        cols.insert("in".to_string(), &data[..]);
        let inputs = KernelInputs::new(cols);
        let params = Params::new();

        let err = check_double_run(counter_kernel, &inputs, &params).unwrap_err();
        assert_eq!(err.0, "out");
        assert_eq!(err.1, 0);
    }
}
