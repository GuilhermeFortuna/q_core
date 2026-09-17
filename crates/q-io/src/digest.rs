//! File digest verification against catalog manifests.

use std::fs::File;
use std::io::Read;
use std::path::Path;

use sha2::{Digest, Sha256};

use crate::error::IoError;

/// Supported digest algorithms as declared in dataset manifests.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DigestAlgorithm {
    Sha256,
}

impl DigestAlgorithm {
    /// Parses a manifest's `checksum_algorithm`; anything else is
    /// `UnsupportedDigest`, so a caller fails closed.
    pub fn parse(name: &str) -> Result<Self, IoError> {
        match name {
            "sha256" => Ok(Self::Sha256),
            other => Err(IoError::UnsupportedDigest {
                algorithm: other.to_string(),
            }),
        }
    }

    /// Canonical lowercase name matching manifest declaration.
    pub fn name(self) -> &'static str {
        match self {
            Self::Sha256 => "sha256",
        }
    }
}

/// Streams the file and returns the lowercase hex digest the manifest carries.
pub fn digest_file(path: &Path, algorithm: DigestAlgorithm) -> Result<String, IoError> {
    let mut file = File::open(path).map_err(|_| IoError::FileMissing {
        path: path.display().to_string(),
    })?;

    match algorithm {
        DigestAlgorithm::Sha256 => {
            let mut hasher = Sha256::new();
            let mut buffer = [0u8; 65536];
            loop {
                let bytes_read = file.read(&mut buffer).map_err(|e| IoError::Arrow {
                    detail: format!("failed to read file `{}`: {e}", path.display()),
                })?;
                if bytes_read == 0 {
                    break;
                }
                hasher.update(&buffer[..bytes_read]);
            }
            let result = hasher.finalize();
            Ok(format!("{result:x}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn parse_valid_algorithm() {
        assert_eq!(
            DigestAlgorithm::parse("sha256").unwrap(),
            DigestAlgorithm::Sha256
        );
        assert_eq!(DigestAlgorithm::Sha256.name(), "sha256");
    }

    #[test]
    fn parse_unsupported_algorithms() {
        for algo in ["blake3", "md5", "SHA-256", "sha512", "unknown"] {
            match DigestAlgorithm::parse(algo) {
                Err(IoError::UnsupportedDigest { algorithm }) => {
                    assert_eq!(algorithm, algo);
                }
                other => panic!("expected UnsupportedDigest for `{algo}`, got {other:?}"),
            }
        }
    }

    #[test]
    fn digest_empty_file() {
        let temp_dir = tempfile::tempdir().unwrap();
        let file_path = temp_dir.path().join("empty.bin");
        File::create(&file_path).unwrap();

        let hex = digest_file(&file_path, DigestAlgorithm::Sha256).unwrap();
        assert_eq!(
            hex,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn digest_known_small_file() {
        let temp_dir = tempfile::tempdir().unwrap();
        let file_path = temp_dir.path().join("small.txt");
        let mut file = File::create(&file_path).unwrap();
        file.write_all(b"hello world\n").unwrap();
        drop(file);

        let hex = digest_file(&file_path, DigestAlgorithm::Sha256).unwrap();
        assert_eq!(
            hex,
            "a948904f2f0f479b8f8197694b30184b0d2ed1c1cd2a1ec0fb85d299a192a447"
        );
    }

    #[test]
    fn digest_missing_file_reports_file_missing() {
        let missing = Path::new("/nonexistent/path/file.bin");
        match digest_file(missing, DigestAlgorithm::Sha256) {
            Err(IoError::FileMissing { path }) => {
                assert!(path.contains("nonexistent"));
            }
            other => panic!("expected FileMissing, got {other:?}"),
        }
    }
}
