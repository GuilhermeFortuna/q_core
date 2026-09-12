#![forbid(unsafe_code)]
//! Pure, deterministic technical indicator mathematics and streaming indicator state.
//!
//! Evaluates technical indicators over numerical series with no I/O, no system clock
//! or environment access, and strictly reproducible results across runs.

/// The crate's build identity. The only public item until the first kernel lands.
pub const CRATE_NAME: &str = "q-indicators";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_crate_name() {
        assert_eq!(CRATE_NAME, "q-indicators");
    }
}
