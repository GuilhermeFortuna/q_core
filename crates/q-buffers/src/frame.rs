//! Columnar bar frames: contracted bar columns plus named derived columns.

use crate::column::{Column, ColumnType};

/// Which wall clock the stored microsecond counts are. No conversion is ever applied.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimeLabel {
    BrasiliaWallclock,
    Utc,
}

impl TimeLabel {
    pub fn contract_marker(self) -> &'static str {
        match self {
            Self::BrasiliaWallclock => "naive-wallclock-America/Sao_Paulo",
            Self::Utc => "UTC",
        }
    }

    pub fn parse(marker: &str) -> Result<Self, FrameError> {
        match marker {
            "naive-wallclock-America/Sao_Paulo" => Ok(Self::BrasiliaWallclock),
            "UTC" => Ok(Self::Utc),
            other => Err(FrameError::UnknownTimeLabel(other.to_string())),
        }
    }
}

/// Contracted bar columns, in any order; validated into a [`BarFrame`] or merged by a window.
#[derive(Clone, Debug)]
pub struct BarColumns {
    pub time: Vec<i64>,
    pub open: Vec<f64>,
    pub high: Vec<f64>,
    pub low: Vec<f64>,
    pub close: Vec<f64>,
    pub tick_volume: Option<Vec<i64>>,
    pub spread: Option<Vec<i64>>,
    pub real_volume: Option<Vec<i64>>,
    pub label: TimeLabel,
}

/// Which optional contracted volume columns are present.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VolumeSet {
    pub tick_volume: bool,
    pub spread: bool,
    pub real_volume: bool,
}

impl VolumeSet {
    pub fn from_bars(bars: &BarColumns) -> Self {
        Self {
            tick_volume: bars.tick_volume.is_some(),
            spread: bars.spread.is_some(),
            real_volume: bars.real_volume.is_some(),
        }
    }
}

/// One field description, shaped like a q_contracts Arrow schema field.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FieldDesc {
    pub name: String,
    pub arrow_type: &'static str,
    pub nullable: bool,
    pub tz: Option<&'static str>,
}

/// Contracted column names are reserved and cannot be used for derived columns.
pub const RESERVED_COLUMNS: [&str; 8] = [
    "time",
    "open",
    "high",
    "low",
    "close",
    "tick_volume",
    "spread",
    "real_volume",
];

/// Immutable-shape columnar bar frame: strictly increasing open times, bar columns,
/// then derived columns in insertion order.
#[derive(Clone, Debug)]
pub struct BarFrame {
    bars: BarColumns,
    derived: Vec<(String, Column)>,
}

impl BarFrame {
    pub fn try_new(bars: BarColumns) -> Result<Self, FrameError> {
        validate_bars(&bars)?;
        Ok(Self {
            bars,
            derived: Vec::new(),
        })
    }

    pub fn len(&self) -> usize {
        self.bars.time.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn label(&self) -> TimeLabel {
        self.bars.label
    }

    pub fn volume_set(&self) -> VolumeSet {
        VolumeSet::from_bars(&self.bars)
    }

    pub fn time(&self) -> &[i64] {
        &self.bars.time
    }

    pub fn open(&self) -> &[f64] {
        &self.bars.open
    }

    pub fn high(&self) -> &[f64] {
        &self.bars.high
    }

    pub fn low(&self) -> &[f64] {
        &self.bars.low
    }

    pub fn close(&self) -> &[f64] {
        &self.bars.close
    }

    pub fn tick_volume(&self) -> Option<&[i64]> {
        self.bars.tick_volume.as_deref()
    }

    pub fn spread(&self) -> Option<&[i64]> {
        self.bars.spread.as_deref()
    }

    pub fn real_volume(&self) -> Option<&[i64]> {
        self.bars.real_volume.as_deref()
    }

    pub fn bars(&self) -> &BarColumns {
        &self.bars
    }

    /// Mutable access for window append/drain. Windows never carry derived columns.
    pub(crate) fn bars_mut(&mut self) -> &mut BarColumns {
        &mut self.bars
    }

    pub fn push_column(&mut self, name: &str, column: Column) -> Result<(), FrameError> {
        if name.is_empty() {
            return Err(FrameError::EmptyName);
        }
        if RESERVED_COLUMNS.contains(&name) {
            return Err(FrameError::ReservedName(name.to_string()));
        }
        if self.derived.iter().any(|(n, _)| n == name) {
            return Err(FrameError::DuplicateName(name.to_string()));
        }
        let expected = self.len();
        let actual = column.len();
        if actual != expected {
            return Err(FrameError::LengthMismatch {
                column: name.to_string(),
                expected,
                actual,
            });
        }
        self.derived.push((name.to_string(), column));
        Ok(())
    }

    pub fn column(&self, name: &str) -> Option<&Column> {
        self.derived.iter().find(|(n, _)| n == name).map(|(_, c)| c)
    }

    /// Bar columns present (contract order), then derived columns in insertion order.
    pub fn column_names(&self) -> Vec<&str> {
        let mut names = Vec::with_capacity(8 + self.derived.len());
        names.extend(["time", "open", "high", "low", "close"]);
        if self.bars.tick_volume.is_some() {
            names.push("tick_volume");
        }
        if self.bars.spread.is_some() {
            names.push("spread");
        }
        if self.bars.real_volume.is_some() {
            names.push("real_volume");
        }
        for (name, _) in &self.derived {
            names.push(name.as_str());
        }
        names
    }

    /// Contracted field descriptions for present bar columns, matching the Arrow schema shape.
    pub fn schema(&self) -> Vec<FieldDesc> {
        let mut fields = Vec::new();
        fields.push(FieldDesc {
            name: "time".to_string(),
            arrow_type: ColumnType::TimestampMicros.arrow_type(),
            nullable: false,
            tz: Some(self.bars.label.contract_marker()),
        });
        for name in ["open", "high", "low", "close"] {
            fields.push(FieldDesc {
                name: name.to_string(),
                arrow_type: ColumnType::Float64.arrow_type(),
                nullable: false,
                tz: None,
            });
        }
        if self.bars.tick_volume.is_some() {
            fields.push(FieldDesc {
                name: "tick_volume".to_string(),
                arrow_type: ColumnType::Int64.arrow_type(),
                nullable: false,
                tz: None,
            });
        }
        if self.bars.spread.is_some() {
            fields.push(FieldDesc {
                name: "spread".to_string(),
                arrow_type: ColumnType::Int64.arrow_type(),
                nullable: false,
                tz: None,
            });
        }
        if self.bars.real_volume.is_some() {
            fields.push(FieldDesc {
                name: "real_volume".to_string(),
                arrow_type: ColumnType::Int64.arrow_type(),
                nullable: false,
                tz: None,
            });
        }
        fields
    }
}

fn validate_bars(bars: &BarColumns) -> Result<(), FrameError> {
    let n = bars.time.len();
    check_len("open", bars.open.len(), n)?;
    check_len("high", bars.high.len(), n)?;
    check_len("low", bars.low.len(), n)?;
    check_len("close", bars.close.len(), n)?;
    if let Some(ref v) = bars.tick_volume {
        check_len("tick_volume", v.len(), n)?;
    }
    if let Some(ref v) = bars.spread {
        check_len("spread", v.len(), n)?;
    }
    if let Some(ref v) = bars.real_volume {
        check_len("real_volume", v.len(), n)?;
    }
    for i in 1..n {
        if bars.time[i] <= bars.time[i - 1] {
            return Err(FrameError::NonIncreasingTime { index: i });
        }
    }
    Ok(())
}

fn check_len(column: &str, actual: usize, expected: usize) -> Result<(), FrameError> {
    if actual != expected {
        Err(FrameError::LengthMismatch {
            column: column.to_string(),
            expected,
            actual,
        })
    } else {
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FrameError {
    LengthMismatch {
        column: String,
        expected: usize,
        actual: usize,
    },
    NonIncreasingTime {
        index: usize,
    },
    ReservedName(String),
    DuplicateName(String),
    EmptyName,
    UnknownTimeLabel(String),
    VolumeSetMismatch {
        expected: VolumeSet,
        actual: VolumeSet,
    },
    LabelMismatch {
        expected: TimeLabel,
        actual: TimeLabel,
    },
    ZeroBound,
    StaleForming {
        forming_time: i64,
        newest_completed: i64,
    },
    FormingNotSingleBar {
        rows: usize,
    },
}

impl std::fmt::Display for FrameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::LengthMismatch {
                column,
                expected,
                actual,
            } => write!(
                f,
                "column `{column}` has length {actual}, expected {expected}"
            ),
            Self::NonIncreasingTime { index } => {
                write!(f, "bar times are not strictly increasing at index {index}")
            }
            Self::ReservedName(name) => write!(f, "column name `{name}` is reserved"),
            Self::DuplicateName(name) => write!(f, "duplicate column name `{name}`"),
            Self::EmptyName => write!(f, "column name must be non-empty"),
            Self::UnknownTimeLabel(label) => write!(f, "unknown time label `{label}`"),
            Self::VolumeSetMismatch { expected, actual } => write!(
                f,
                "volume set mismatch: expected {expected:?}, got {actual:?}"
            ),
            Self::LabelMismatch { expected, actual } => {
                write!(
                    f,
                    "time label mismatch: expected {expected:?}, got {actual:?}"
                )
            }
            Self::ZeroBound => write!(f, "rolling window bound must be at least 1"),
            Self::StaleForming {
                forming_time,
                newest_completed,
            } => write!(
                f,
                "forming bar time {forming_time} is not after newest completed {newest_completed}"
            ),
            Self::FormingNotSingleBar { rows } => {
                write!(f, "forming bar must have exactly one row, got {rows}")
            }
        }
    }
}

impl std::error::Error for FrameError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::column::{Bitmap, Column};

    fn sample_bars(n: usize) -> BarColumns {
        let hour = 3_600_000_000i64;
        BarColumns {
            time: (0..n as i64).map(|i| i * hour).collect(),
            open: vec![1.0; n],
            high: vec![2.0; n],
            low: vec![0.5; n],
            close: vec![1.5; n],
            tick_volume: None,
            spread: None,
            real_volume: None,
            label: TimeLabel::Utc,
        }
    }

    #[test]
    fn try_new_accepts_strictly_increasing_times() {
        let frame = BarFrame::try_new(sample_bars(3)).unwrap();
        assert_eq!(frame.len(), 3);
    }

    #[test]
    fn try_new_rejects_non_increasing_times() {
        let mut bars = sample_bars(2);
        bars.time = vec![0, 0];
        assert_eq!(
            BarFrame::try_new(bars).unwrap_err(),
            FrameError::NonIncreasingTime { index: 1 }
        );
    }

    #[test]
    fn try_new_rejects_length_mismatch() {
        let mut bars = sample_bars(3);
        bars.close = vec![1.0, 2.0];
        assert_eq!(
            BarFrame::try_new(bars).unwrap_err(),
            FrameError::LengthMismatch {
                column: "close".to_string(),
                expected: 3,
                actual: 2,
            }
        );
    }

    #[test]
    fn push_column_rejects_reserved_duplicate_and_empty() {
        let mut frame = BarFrame::try_new(sample_bars(2)).unwrap();
        let col = Column::Float64(vec![1.0, 2.0]);
        assert_eq!(
            frame.push_column("close", col.clone()).unwrap_err(),
            FrameError::ReservedName("close".to_string())
        );
        frame.push_column("rsi", col.clone()).unwrap();
        assert_eq!(
            frame.push_column("rsi", col).unwrap_err(),
            FrameError::DuplicateName("rsi".to_string())
        );
        assert_eq!(
            frame
                .push_column("", Column::Float64(vec![0.0, 0.0]))
                .unwrap_err(),
            FrameError::EmptyName
        );
    }

    #[test]
    fn column_names_preserve_bar_then_insertion_order() {
        let mut bars = sample_bars(2);
        bars.tick_volume = Some(vec![1, 2]);
        let mut frame = BarFrame::try_new(bars).unwrap();
        frame
            .push_column("rsi", Column::Float64(vec![1.0, 2.0]))
            .unwrap();
        frame
            .push_column(
                "buy_signal",
                Column::Bool(Bitmap::from_bools(&[true, false])),
            )
            .unwrap();
        frame
            .push_column("bar_index", Column::Int64(vec![0, 1]))
            .unwrap();
        assert_eq!(
            frame.column_names(),
            vec![
                "time",
                "open",
                "high",
                "low",
                "close",
                "tick_volume",
                "rsi",
                "buy_signal",
                "bar_index",
            ]
        );
    }

    #[test]
    fn time_label_parse_rejects_unknown() {
        assert_eq!(
            TimeLabel::parse("America/Sao_Paulo").unwrap_err(),
            FrameError::UnknownTimeLabel("America/Sao_Paulo".to_string())
        );
    }

    #[test]
    fn schema_matches_vendored_bars_contract() {
        let raw = include_str!("../../../contracts/schema/api/arrow/bars.schema.json");
        let json: serde_json::Value = serde_json::from_str(raw).unwrap();
        let contract_fields = json["fields"].as_array().unwrap();

        let bars = BarColumns {
            time: vec![0],
            open: vec![1.0],
            high: vec![1.0],
            low: vec![1.0],
            close: vec![1.0],
            tick_volume: Some(vec![1]),
            spread: Some(vec![0]),
            real_volume: Some(vec![0]),
            label: TimeLabel::BrasiliaWallclock,
        };
        let frame = BarFrame::try_new(bars).unwrap();
        let schema = frame.schema();
        assert_eq!(schema.len(), contract_fields.len());
        for (desc, field) in schema.iter().zip(contract_fields.iter()) {
            assert_eq!(desc.name, field["name"].as_str().unwrap());
            assert_eq!(desc.arrow_type, field["type"].as_str().unwrap());
            assert_eq!(desc.nullable, field["nullable"].as_bool().unwrap());
            let expected_tz = field.get("tz").and_then(|v| v.as_str());
            assert_eq!(desc.tz, expected_tz);
        }
        assert_eq!(
            TimeLabel::BrasiliaWallclock.contract_marker(),
            contract_fields[0]["tz"].as_str().unwrap()
        );
    }
}
