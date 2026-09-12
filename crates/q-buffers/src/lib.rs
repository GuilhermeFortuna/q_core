#![forbid(unsafe_code)]
//! Columnar ring buffers, bar aggregation buffers, and Arrow memory layout abstractions.
//!
//! Holds in-memory market series and streaming bar windows for high-throughput numeric evaluation
//! without disk access or catalog discovery.

/// The crate's build identity. The only public item until the first kernel lands.
pub const CRATE_NAME: &str = "q-buffers";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_crate_name() {
        assert_eq!(CRATE_NAME, "q-buffers");
    }
}
