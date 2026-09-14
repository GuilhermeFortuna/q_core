#![forbid(unsafe_code)]

//! Test support for the reference parity gate.
//! Reads reference fixtures, compares outputs under a declared policy,
//! and checks double-run determinism and prefix causality.
//! Dev-dependency only; never linked into q-py or q-qt.

pub mod compare;
pub mod fixture;
pub mod gate;
