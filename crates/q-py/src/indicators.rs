//! Temporary projection spike for Q-022 step 2 (`identity`). Removed in step 13.

use numpy::{PyArray1, PyReadonlyArray1};
use pyo3::prelude::*;
use pyo3::types::PyModule;
use pyo3::Bound;

/// Round-trip a contiguous float64 array to verify abi3 + rust-numpy wiring.
#[pyfunction]
fn identity<'py>(
    py: Python<'py>,
    values: PyReadonlyArray1<'py, f64>,
) -> PyResult<Bound<'py, PyArray1<f64>>> {
    let slice = values.as_slice()?;
    Ok(PyArray1::from_vec(py, slice.to_vec()))
}

pub(crate) fn register(parent: &Bound<'_, PyModule>) -> PyResult<()> {
    let m = PyModule::new(parent.py(), "indicators")?;
    m.add_function(wrap_pyfunction!(identity, &m)?)?;
    parent.add_submodule(&m)?;
    parent
        .py()
        .import("sys")?
        .getattr("modules")?
        .set_item("q_core.indicators", &m)?;
    Ok(())
}
