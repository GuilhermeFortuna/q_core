#![forbid(unsafe_code)]
//! Pure Parquet and Arrow columnar codecs and byte readers.
//!
//! Reads and decodes explicitly provided columnar files and byte streams without performing
//! filesystem discovery or managing dataset lifecycles.

pub mod digest;
pub mod error;

pub use digest::{digest_file, DigestAlgorithm};
pub use error::IoError;

/// The crate's build identity. The only public item until the first kernel lands.
pub const CRATE_NAME: &str = "q-io";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_crate_name() {
        assert_eq!(CRATE_NAME, "q-io");
    }
}
