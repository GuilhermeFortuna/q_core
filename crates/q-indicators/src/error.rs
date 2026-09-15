//! Indicator parameter and input validation errors.

/// A parameter or input rejected by a public indicator kernel.
#[derive(Debug, Clone, PartialEq)]
pub enum IndicatorError {
    /// A parameter outside the domain the Python reference accepts.
    InvalidParameter {
        function: &'static str,
        parameter: &'static str,
        value: i64,
        requirement: &'static str,
    },
    /// `clip`: low > high (both non-NaN).
    InvalidBounds { low: f64, high: f64 },
    /// Price columns that must align have different lengths.
    LengthMismatch {
        function: &'static str,
        parameter: &'static str,
        expected: usize,
        actual: usize,
    },
}

impl core::fmt::Display for IndicatorError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::InvalidParameter {
                function,
                parameter,
                value,
                requirement,
            } => write!(
                f,
                "{function}: {parameter} must be {requirement} (got {value})"
            ),
            Self::InvalidBounds { low, high } => {
                write!(f, "clip: low must be <= high (got {low} > {high})")
            }
            Self::LengthMismatch {
                function,
                parameter,
                expected,
                actual,
            } => write!(f, "{function}: {parameter} length {actual} != {expected}"),
        }
    }
}

impl std::error::Error for IndicatorError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_parameter_display_matches_plan() {
        let err = IndicatorError::InvalidParameter {
            function: "rsi",
            parameter: "period",
            value: 0,
            requirement: ">= 1",
        };
        assert_eq!(err.to_string(), "rsi: period must be >= 1 (got 0)");
    }
}
