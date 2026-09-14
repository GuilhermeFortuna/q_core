#![forbid(unsafe_code)]

//! Prefix causality verification.

use crate::compare::{compare_f64, Policy};
use crate::fixture::Params;
use crate::gate::{KernelFn, KernelInputs};

#[derive(Debug, Clone, PartialEq)]
pub struct NonCausalAt {
    pub output: String,
    pub index: usize,
    pub full: f64,
    pub prefix: f64,
}

pub fn check_prefix_causal(
    kernel: KernelFn,
    inputs: &KernelInputs<'_>,
    params: &Params,
    policy: &Policy,
) -> Result<(), NonCausalAt> {
    let full_out = kernel(inputs, params).map_err(|_| NonCausalAt {
        output: "kernel_error".to_string(),
        index: 0,
        full: 0.0,
        prefix: 0.0,
    })?;

    for len in 1..=inputs.len() {
        let prefix_inputs = inputs.prefix(len);
        let prefix_out = kernel(&prefix_inputs, params).map_err(|_| NonCausalAt {
            output: "kernel_error".to_string(),
            index: 0,
            full: 0.0,
            prefix: 0.0,
        })?;

        for (name, prefix_vec) in &prefix_out.0 {
            let full_vec = match full_out.0.get(name) {
                Some(v) => v,
                None => {
                    return Err(NonCausalAt {
                        output: name.clone(),
                        index: 0,
                        full: 0.0,
                        prefix: 0.0,
                    });
                }
            };

            for (i, &p_val) in prefix_vec.iter().enumerate() {
                let f_val = full_vec[i];
                if compare_f64(policy, &[f_val], &[p_val]).is_err() {
                    return Err(NonCausalAt {
                        output: name.clone(),
                        index: i,
                        full: f_val,
                        prefix: p_val,
                    });
                }
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::determinism::check_double_run;
    use crate::gate::{KernelError, KernelOutputs};
    use std::collections::BTreeMap;

    fn identity_kernel(
        inputs: &KernelInputs<'_>,
        _params: &Params,
    ) -> Result<KernelOutputs, KernelError> {
        let col = inputs.column("in")?;
        let mut out = KernelOutputs::new();
        out.insert("out", col.to_vec());
        Ok(out)
    }

    fn next_bar_kernel(
        inputs: &KernelInputs<'_>,
        _params: &Params,
    ) -> Result<KernelOutputs, KernelError> {
        let col = inputs.column("in")?;
        let n = col.len();
        let mut out_vec = Vec::with_capacity(n);
        for i in 0..n {
            if i + 1 < n {
                out_vec.push(col[i + 1]);
            } else {
                out_vec.push(f64::NAN);
            }
        }
        let mut out = KernelOutputs::new();
        out.insert("out", out_vec);
        Ok(out)
    }

    #[test]
    fn test_identity_kernel_passes_causality() {
        let data = [1.0, 2.0, 3.0];
        let mut cols = BTreeMap::new();
        cols.insert("in".to_string(), &data[..]);
        let inputs = KernelInputs::new(cols);
        let params = Params::new();
        let policy = Policy::AbsRelTol {
            abs: 1e-10,
            rel: 1e-12,
        };

        assert!(check_prefix_causal(identity_kernel, &inputs, &params, &policy).is_ok());
    }

    #[test]
    fn test_next_bar_kernel_passes_double_run_and_fails_causality_at_zero() {
        let data = [10.0, 20.0, 30.0];
        let mut cols = BTreeMap::new();
        cols.insert("in".to_string(), &data[..]);
        let inputs = KernelInputs::new(cols);
        let params = Params::new();
        let policy = Policy::AbsRelTol {
            abs: 1e-10,
            rel: 1e-12,
        };

        // Passes double-run determinism
        assert!(check_double_run(next_bar_kernel, &inputs, &params).is_ok());

        // Fails prefix causality at index 0
        let err = check_prefix_causal(next_bar_kernel, &inputs, &params, &policy).unwrap_err();
        assert_eq!(err.output, "out");
        assert_eq!(err.index, 0);
        assert_eq!(err.full.to_bits(), 20.0f64.to_bits());
        assert!(err.prefix.is_nan());
    }
}
