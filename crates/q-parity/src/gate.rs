#![forbid(unsafe_code)]

//! Reference parity gate definitions.

use std::collections::BTreeMap;
use std::fmt;

use crate::fixture::Params;

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
