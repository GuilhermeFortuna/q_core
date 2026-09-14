#![forbid(unsafe_code)]
//! Pure, deterministic technical indicator mathematics and streaming indicator state.
//!
//! Evaluates technical indicators over numerical series with no I/O, no system clock
//! or environment access, and strictly reproducible results across runs.

mod elementwise;
mod error;
mod ieee;
mod window;

pub use error::IndicatorError;

/// The crate's build identity.
pub const CRATE_NAME: &str = "q-indicators";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_crate_name() {
        assert_eq!(CRATE_NAME, "q-indicators");
    }
}
