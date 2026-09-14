//! Physical column buffers with Arrow-compatible layouts.

/// Physical type of a column, named with the q_contracts Arrow type strings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColumnType {
    TimestampMicros,
    Float64,
    Int64,
    Int8,
    Bool,
}

impl ColumnType {
    /// Arrow type string as declared in q_contracts schemas.
    pub fn arrow_type(self) -> &'static str {
        match self {
            Self::TimestampMicros => "timestamp[us]",
            Self::Float64 => "float64",
            Self::Int64 => "int64",
            Self::Int8 => "int8",
            Self::Bool => "bool",
        }
    }
}

/// Bit-packed booleans, LSB first, trailing bits zero (Arrow boolean layout).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bitmap {
    bytes: Vec<u8>,
    len: usize,
}

impl Bitmap {
    /// Pack `values` LSB-first; unused trailing bits in the last byte are zero.
    pub fn from_bools(values: &[bool]) -> Self {
        let nbytes = values.len().div_ceil(8);
        let mut bytes = vec![0u8; nbytes];
        for (i, &v) in values.iter().enumerate() {
            if v {
                bytes[i / 8] |= 1u8 << (i % 8);
            }
        }
        Self {
            bytes,
            len: values.len(),
        }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Return the boolean at `index`.
    ///
    /// # Panics
    ///
    /// Panics if `index >= self.len()`.
    pub fn get(&self, index: usize) -> bool {
        assert!(
            index < self.len,
            "bitmap index {index} out of range {}",
            self.len
        );
        (self.bytes[index / 8] & (1u8 << (index % 8))) != 0
    }

    /// Underlying Arrow boolean buffer (`ceil(len / 8)` bytes).
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn to_bools(&self) -> Vec<bool> {
        (0..self.len).map(|i| self.get(i)).collect()
    }
}

/// One owned column buffer. Float NaN = missing; other types have no missing values.
#[derive(Clone, Debug)]
pub enum Column {
    Float64(Vec<f64>),
    Int64(Vec<i64>),
    Int8(Vec<i8>),
    Bool(Bitmap),
}

impl Column {
    pub fn len(&self) -> usize {
        match self {
            Self::Float64(v) => v.len(),
            Self::Int64(v) => v.len(),
            Self::Int8(v) => v.len(),
            Self::Bool(b) => b.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn column_type(&self) -> ColumnType {
        match self {
            Self::Float64(_) => ColumnType::Float64,
            Self::Int64(_) => ColumnType::Int64,
            Self::Int8(_) => ColumnType::Int8,
            Self::Bool(_) => ColumnType::Bool,
        }
    }

    /// Bitwise equality, including NaN payloads. Lint-safe float comparison.
    pub fn bytes_eq(&self, other: &Column) -> bool {
        match (self, other) {
            (Self::Float64(a), Self::Float64(b)) => {
                a.len() == b.len()
                    && a.iter()
                        .zip(b.iter())
                        .all(|(x, y)| x.to_bits() == y.to_bits())
            }
            (Self::Int64(a), Self::Int64(b)) => a == b,
            (Self::Int8(a), Self::Int8(b)) => a == b,
            (Self::Bool(a), Self::Bool(b)) => a == b,
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bitmap_packs_lsb_first_with_trailing_zeros() {
        let values = [
            true, false, true, true, false, false, false, false, true, true,
        ];
        let bitmap = Bitmap::from_bools(&values);
        assert_eq!(bitmap.as_bytes(), &[0b0000_1101, 0b0000_0011]);
        assert!(bitmap.get(9));
        assert_eq!(bitmap.to_bools(), values.to_vec());
    }

    #[test]
    fn column_type_arrow_names() {
        assert_eq!(ColumnType::Float64.arrow_type(), "float64");
        assert_eq!(ColumnType::TimestampMicros.arrow_type(), "timestamp[us]");
    }

    #[test]
    fn float_column_bytes_eq_handles_nan_and_signed_zero() {
        let nan = Column::Float64(vec![f64::NAN]);
        assert!(nan.bytes_eq(&Column::Float64(vec![f64::NAN])));
        assert!(!Column::Float64(vec![0.0]).bytes_eq(&Column::Float64(vec![-0.0])));
    }
}
