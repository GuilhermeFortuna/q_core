use pyo3::prelude::*;

mod frame;

/// Workspace version, read from the workspace manifest at compile time.
#[pyfunction]
fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// The contracts commit this build vendored, read from CONTRACTS_REV at compile time.
#[pyfunction]
fn contracts_rev() -> &'static str {
    include_str!("../../../CONTRACTS_REV").trim()
}

/// Python bindings for the q_core workspace.
#[pymodule]
fn q_core(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(version, m)?)?;
    m.add_function(wrap_pyfunction!(contracts_rev, m)?)?;
    frame::register(m)?;
    Ok(())
}
