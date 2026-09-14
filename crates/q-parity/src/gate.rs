#![forbid(unsafe_code)]

//! Reference parity gate definitions.

use std::collections::BTreeMap;

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
