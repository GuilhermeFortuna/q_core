#![forbid(unsafe_code)]
//! Columnar ring buffers, bar aggregation buffers, and Arrow memory layout abstractions.
//!
//! Holds in-memory market series and streaming bar windows for high-throughput numeric evaluation
//! without disk access or catalog discovery.

/// Vendored contract types for cross-process wire and stream shapes.
#[allow(non_snake_case)]
#[path = "../../../contracts"]
pub mod contracts {
    pub mod api;
    pub mod catalog;
    pub mod stream;
}

/// The crate's build identity. The only public item until the first kernel lands.
pub const CRATE_NAME: &str = "q-buffers";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_crate_name() {
        assert_eq!(CRATE_NAME, "q-buffers");
    }

    #[test]
    fn test_contracts_available() {
        let frame = contracts::stream::LaggingFrame {
            from_seq: 42,
            topic: "q.test".to_string(),
        };
        assert_eq!(frame.from_seq, 42);
        assert_eq!(frame.topic, "q.test");
    }
}
