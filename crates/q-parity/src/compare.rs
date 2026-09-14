#![forbid(unsafe_code)]

//! Numeric comparison policies for the reference parity gate.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Policy {
    AbsRelTol { abs: f64, rel: f64 },
    Exact,
}
