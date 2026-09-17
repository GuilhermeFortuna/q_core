//! Error types for columnar codecs and readers.

/// Errors produced when decoding Arrow IPC streams or reading Parquet files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IoError {
    UnexpectedColumn {
        name: String,
    },
    MissingColumn {
        name: String,
    },
    ColumnType {
        name: String,
        expected: &'static str,
        found: String,
    },
    NullValue {
        file: Option<String>,
        column: String,
        row: usize,
    },
    NotParquet {
        path: String,
    },
    PathPattern {
        path: String,
    },
    FileMissing {
        path: String,
    },
    RowRange {
        start: usize,
        end: usize,
        rows: usize,
    },
    UnsupportedDigest {
        algorithm: String,
    },
    Arrow {
        detail: String,
    },
    Frame(q_buffers::FrameError),
}

impl std::fmt::Display for IoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnexpectedColumn { name } => {
                write!(f, "unexpected column `{name}`")
            }
            Self::MissingColumn { name } => {
                write!(f, "missing contracted column `{name}`")
            }
            Self::ColumnType {
                name,
                expected,
                found,
            } => {
                write!(
                    f,
                    "column `{name}` has type `{found}`, expected `{expected}`"
                )
            }
            Self::NullValue {
                file: Some(file),
                column,
                row,
            } => {
                write!(
                    f,
                    "null value in file `{file}`, column `{column}` at row {row}"
                )
            }
            Self::NullValue {
                file: None,
                column,
                row,
            } => {
                write!(f, "null value in column `{column}` at row {row}")
            }
            Self::NotParquet { path } => {
                write!(f, "not a Parquet file: `{path}`")
            }
            Self::PathPattern { path } => {
                write!(f, "path contains pattern character: `{path}`")
            }
            Self::FileMissing { path } => {
                write!(f, "file missing: `{path}`")
            }
            Self::RowRange { start, end, rows } => {
                write!(
                    f,
                    "row range [{start}, {end}) out of bounds for {rows} rows"
                )
            }
            Self::UnsupportedDigest { algorithm } => {
                write!(f, "unsupported digest algorithm `{algorithm}`")
            }
            Self::Arrow { detail } => {
                write!(f, "arrow error: {detail}")
            }
            Self::Frame(err) => {
                write!(f, "{err}")
            }
        }
    }
}

impl std::error::Error for IoError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Frame(err) => Some(err),
            _ => None,
        }
    }
}

impl From<q_buffers::FrameError> for IoError {
    fn from(err: q_buffers::FrameError) -> Self {
        Self::Frame(err)
    }
}

impl From<arrow::error::ArrowError> for IoError {
    fn from(err: arrow::error::ArrowError) -> Self {
        Self::Arrow {
            detail: err.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_names_column_for_unexpected_column() {
        let err = IoError::UnexpectedColumn {
            name: "volume_ratio".to_string(),
        };
        let msg = err.to_string();
        assert!(msg.contains("volume_ratio"));
    }

    #[test]
    fn display_names_column_for_missing_column() {
        let err = IoError::MissingColumn {
            name: "close".to_string(),
        };
        let msg = err.to_string();
        assert!(msg.contains("close"));
    }

    #[test]
    fn display_names_column_and_types_for_column_type() {
        let err = IoError::ColumnType {
            name: "open".to_string(),
            expected: "float64",
            found: "float32".to_string(),
        };
        let msg = err.to_string();
        assert!(msg.contains("open"));
        assert!(msg.contains("float64"));
        assert!(msg.contains("float32"));
    }

    #[test]
    fn display_names_file_column_and_row_for_null_value() {
        let err_with_file = IoError::NullValue {
            file: Some("2026-03.parquet".to_string()),
            column: "high".to_string(),
            row: 42,
        };
        let msg = err_with_file.to_string();
        assert!(msg.contains("2026-03.parquet"));
        assert!(msg.contains("high"));
        assert!(msg.contains("42"));

        let err_no_file = IoError::NullValue {
            file: None,
            column: "high".to_string(),
            row: 17,
        };
        let msg_no_file = err_no_file.to_string();
        assert!(msg_no_file.contains("high"));
        assert!(msg_no_file.contains("17"));
    }

    #[test]
    fn display_names_path_for_not_parquet() {
        let err = IoError::NotParquet {
            path: "data/not_a_parquet.csv".to_string(),
        };
        let msg = err.to_string();
        assert!(msg.contains("data/not_a_parquet.csv"));
    }

    #[test]
    fn display_names_path_for_path_pattern() {
        let err = IoError::PathPattern {
            path: "data/*.parquet".to_string(),
        };
        let msg = err.to_string();
        assert!(msg.contains("data/*.parquet"));
    }

    #[test]
    fn display_names_path_for_file_missing() {
        let err = IoError::FileMissing {
            path: "nonexistent.parquet".to_string(),
        };
        let msg = err.to_string();
        assert!(msg.contains("nonexistent.parquet"));
    }

    #[test]
    fn display_names_range_for_row_range() {
        let err = IoError::RowRange {
            start: 100,
            end: 200,
            rows: 50,
        };
        let msg = err.to_string();
        assert!(msg.contains("100"));
        assert!(msg.contains("200"));
        assert!(msg.contains("50"));
    }

    #[test]
    fn display_names_algorithm_for_unsupported_digest() {
        let err = IoError::UnsupportedDigest {
            algorithm: "blake3".to_string(),
        };
        let msg = err.to_string();
        assert!(msg.contains("blake3"));
    }

    #[test]
    fn display_formats_arrow_and_frame_errors() {
        let arrow_err = IoError::Arrow {
            detail: "stream truncated".to_string(),
        };
        assert!(arrow_err.to_string().contains("stream truncated"));

        let frame_err = IoError::Frame(q_buffers::FrameError::EmptyName);
        assert!(frame_err.to_string().contains("empty"));
    }
}
