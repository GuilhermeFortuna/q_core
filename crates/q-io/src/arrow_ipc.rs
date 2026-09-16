//! Arrow IPC stream decoder into bar frames.

use std::io::Cursor;
use std::sync::LazyLock;

use arrow::array::{Array, Float64Array, Int64Array, TimestampMicrosecondArray};
use arrow::datatypes::{DataType, TimeUnit};
use arrow::ipc::reader::StreamReader;
use q_buffers::{BarColumns, BarFrame, FieldDesc, TimeLabel};

use crate::error::IoError;

static BAR_FIELDS: LazyLock<[FieldDesc; 8]> = LazyLock::new(|| {
    [
        FieldDesc {
            name: "time".to_string(),
            arrow_type: "timestamp[us]",
            nullable: false,
            tz: Some("naive-wallclock-America/Sao_Paulo"),
        },
        FieldDesc {
            name: "open".to_string(),
            arrow_type: "float64",
            nullable: false,
            tz: None,
        },
        FieldDesc {
            name: "high".to_string(),
            arrow_type: "float64",
            nullable: false,
            tz: None,
        },
        FieldDesc {
            name: "low".to_string(),
            arrow_type: "float64",
            nullable: false,
            tz: None,
        },
        FieldDesc {
            name: "close".to_string(),
            arrow_type: "float64",
            nullable: false,
            tz: None,
        },
        FieldDesc {
            name: "tick_volume".to_string(),
            arrow_type: "int64",
            nullable: false,
            tz: None,
        },
        FieldDesc {
            name: "spread".to_string(),
            arrow_type: "int64",
            nullable: false,
            tz: None,
        },
        FieldDesc {
            name: "real_volume".to_string(),
            arrow_type: "int64",
            nullable: false,
            tz: None,
        },
    ]
});

/// The contracted bar field set the decoder and the Parquet reader both enforce,
/// in the order `BarFrame::schema` reports it.
pub fn bar_field_set() -> &'static [FieldDesc] {
    &BAR_FIELDS[..]
}

const REQUIRED_COLUMNS: [&str; 8] = [
    "time",
    "open",
    "high",
    "low",
    "close",
    "tick_volume",
    "spread",
    "real_volume",
];

pub(crate) fn format_arrow_type(dt: &DataType) -> String {
    match dt {
        DataType::Timestamp(TimeUnit::Microsecond, _) => "timestamp[us]".to_string(),
        DataType::Timestamp(TimeUnit::Millisecond, _) => "timestamp[ms]".to_string(),
        DataType::Timestamp(TimeUnit::Second, _) => "timestamp[s]".to_string(),
        DataType::Timestamp(TimeUnit::Nanosecond, _) => "timestamp[ns]".to_string(),
        DataType::Float64 => "float64".to_string(),
        DataType::Float32 => "float32".to_string(),
        DataType::Float16 => "float16".to_string(),
        DataType::Int64 => "int64".to_string(),
        DataType::Int32 => "int32".to_string(),
        DataType::Int16 => "int16".to_string(),
        DataType::Int8 => "int8".to_string(),
        DataType::UInt64 => "uint64".to_string(),
        DataType::UInt32 => "uint32".to_string(),
        DataType::UInt16 => "uint16".to_string(),
        DataType::UInt8 => "uint8".to_string(),
        DataType::Boolean => "bool".to_string(),
        DataType::Utf8 => "string".to_string(),
        DataType::LargeUtf8 => "large_string".to_string(),
        DataType::Binary => "binary".to_string(),
        DataType::LargeBinary => "large_binary".to_string(),
        other => format!("{other}").to_lowercase(),
    }
}

/// Decodes Arrow IPC *stream* bytes into the contracted bar columns.
/// Batches concatenate in order; zero batches give an empty frame.
pub fn decode_bar_batches(bytes: &[u8]) -> Result<BarFrame, IoError> {
    let cursor = Cursor::new(bytes);
    let mut reader = StreamReader::try_new(cursor, None).map_err(|e| IoError::Arrow {
        detail: e.to_string(),
    })?;

    let schema = reader.schema();

    // 1. Check for missing contracted columns (in required order)
    for &col_name in &REQUIRED_COLUMNS {
        if schema.column_with_name(col_name).is_none() {
            return Err(IoError::MissingColumn {
                name: col_name.to_string(),
            });
        }
    }

    // 2. Check for unexpected columns
    for field in schema.fields() {
        if !REQUIRED_COLUMNS.contains(&field.name().as_str()) {
            return Err(IoError::UnexpectedColumn {
                name: field.name().clone(),
            });
        }
    }

    // 3. Check physical types for contracted columns
    for field in schema.fields() {
        let name = field.name().as_str();
        let dt = field.data_type();
        match name {
            "time" => {
                if !matches!(dt, DataType::Timestamp(TimeUnit::Microsecond, _)) {
                    return Err(IoError::ColumnType {
                        name: name.to_string(),
                        expected: "timestamp[us]",
                        found: format_arrow_type(dt),
                    });
                }
            }
            "open" | "high" | "low" | "close" => {
                if !matches!(dt, DataType::Float64) {
                    return Err(IoError::ColumnType {
                        name: name.to_string(),
                        expected: "float64",
                        found: format_arrow_type(dt),
                    });
                }
            }
            "tick_volume" | "spread" | "real_volume" => {
                if !matches!(dt, DataType::Int64) {
                    return Err(IoError::ColumnType {
                        name: name.to_string(),
                        expected: "int64",
                        found: format_arrow_type(dt),
                    });
                }
            }
            _ => unreachable!(),
        }
    }

    let mut time = Vec::new();
    let mut open = Vec::new();
    let mut high = Vec::new();
    let mut low = Vec::new();
    let mut close = Vec::new();
    let mut tick_volume = Vec::new();
    let mut spread = Vec::new();
    let mut real_volume = Vec::new();

    let mut total_rows = 0usize;

    for batch_result in reader.by_ref() {
        let batch = batch_result.map_err(|e| IoError::Arrow {
            detail: e.to_string(),
        })?;

        let time_idx = schema.index_of("time").unwrap();
        let open_idx = schema.index_of("open").unwrap();
        let high_idx = schema.index_of("high").unwrap();
        let low_idx = schema.index_of("low").unwrap();
        let close_idx = schema.index_of("close").unwrap();
        let tick_vol_idx = schema.index_of("tick_volume").unwrap();
        let spread_idx = schema.index_of("spread").unwrap();
        let real_vol_idx = schema.index_of("real_volume").unwrap();

        let time_arr = batch.column(time_idx);
        let open_arr = batch.column(open_idx);
        let high_arr = batch.column(high_idx);
        let low_arr = batch.column(low_idx);
        let close_arr = batch.column(close_idx);
        let tick_vol_arr = batch.column(tick_vol_idx);
        let spread_arr = batch.column(spread_idx);
        let real_vol_arr = batch.column(real_vol_idx);

        let columns: [(&'static str, &arrow::array::ArrayRef); 8] = [
            ("time", time_arr),
            ("open", open_arr),
            ("high", high_arr),
            ("low", low_arr),
            ("close", close_arr),
            ("tick_volume", tick_vol_arr),
            ("spread", spread_arr),
            ("real_volume", real_vol_arr),
        ];

        // Check for nulls, finding the earliest row if any
        let mut earliest_null: Option<(usize, &'static str)> = None;
        for (name, arr) in &columns {
            if arr.null_count() > 0 {
                for i in 0..arr.len() {
                    if arr.is_null(i) {
                        match earliest_null {
                            Some((min_r, _)) if i < min_r => {
                                earliest_null = Some((i, name));
                            }
                            None => {
                                earliest_null = Some((i, name));
                            }
                            _ => {}
                        }
                        break;
                    }
                }
            }
        }

        if let Some((row, col)) = earliest_null {
            return Err(IoError::NullValue {
                file: None,
                column: col.to_string(),
                row: total_rows + row,
            });
        }

        let time_prim = time_arr
            .as_any()
            .downcast_ref::<TimestampMicrosecondArray>()
            .unwrap();
        let open_prim = open_arr.as_any().downcast_ref::<Float64Array>().unwrap();
        let high_prim = high_arr.as_any().downcast_ref::<Float64Array>().unwrap();
        let low_prim = low_arr.as_any().downcast_ref::<Float64Array>().unwrap();
        let close_prim = close_arr.as_any().downcast_ref::<Float64Array>().unwrap();
        let tick_vol_prim = tick_vol_arr.as_any().downcast_ref::<Int64Array>().unwrap();
        let spread_prim = spread_arr.as_any().downcast_ref::<Int64Array>().unwrap();
        let real_vol_prim = real_vol_arr.as_any().downcast_ref::<Int64Array>().unwrap();

        time.extend_from_slice(time_prim.values());
        open.extend_from_slice(open_prim.values());
        high.extend_from_slice(high_prim.values());
        low.extend_from_slice(low_prim.values());
        close.extend_from_slice(close_prim.values());
        tick_volume.extend_from_slice(tick_vol_prim.values());
        spread.extend_from_slice(spread_prim.values());
        real_volume.extend_from_slice(real_vol_prim.values());

        total_rows += batch.num_rows();
    }

    let bars = BarColumns {
        time,
        open,
        high,
        low,
        close,
        tick_volume: Some(tick_volume),
        spread: Some(spread),
        real_volume: Some(real_volume),
        label: TimeLabel::BrasiliaWallclock,
    };

    BarFrame::try_new(bars).map_err(IoError::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use arrow::array::{
        ArrayRef, Float32Array, Float64Array, Int32Array, Int64Array, TimestampMicrosecondArray,
    };
    use arrow::datatypes::{DataType, Field, Schema, TimeUnit};
    use arrow::ipc::writer::StreamWriter;
    use arrow::record_batch::RecordBatch;

    fn make_contract_schema() -> Arc<Schema> {
        Arc::new(Schema::new(vec![
            Field::new(
                "time",
                DataType::Timestamp(
                    TimeUnit::Microsecond,
                    Some("naive-wallclock-America/Sao_Paulo".into()),
                ),
                false,
            ),
            Field::new("open", DataType::Float64, false),
            Field::new("high", DataType::Float64, false),
            Field::new("low", DataType::Float64, false),
            Field::new("close", DataType::Float64, false),
            Field::new("tick_volume", DataType::Int64, false),
            Field::new("spread", DataType::Int64, false),
            Field::new("real_volume", DataType::Int64, false),
        ]))
    }

    fn make_contract_batch(start_time: i64, count: usize) -> RecordBatch {
        let times: Vec<i64> = (0..count)
            .map(|i| start_time + (i as i64) * 60_000_000)
            .collect();
        let opens: Vec<f64> = (0..count).map(|i| 100.0 + i as f64).collect();
        let highs: Vec<f64> = (0..count).map(|i| 105.0 + i as f64).collect();
        let lows: Vec<f64> = (0..count).map(|i| 99.0 + i as f64).collect();
        let closes: Vec<f64> = (0..count).map(|i| 102.0 + i as f64).collect();
        let tick_vols: Vec<i64> = (0..count).map(|i| 10 + i as i64).collect();
        let spreads: Vec<i64> = (0..count).map(|i| 1 + i as i64).collect();
        let real_vols: Vec<i64> = (0..count).map(|i| 1000 + i as i64).collect();

        RecordBatch::try_new(
            make_contract_schema(),
            vec![
                Arc::new(
                    TimestampMicrosecondArray::from(times)
                        .with_timezone("naive-wallclock-America/Sao_Paulo"),
                ) as ArrayRef,
                Arc::new(Float64Array::from(opens)) as ArrayRef,
                Arc::new(Float64Array::from(highs)) as ArrayRef,
                Arc::new(Float64Array::from(lows)) as ArrayRef,
                Arc::new(Float64Array::from(closes)) as ArrayRef,
                Arc::new(Int64Array::from(tick_vols)) as ArrayRef,
                Arc::new(Int64Array::from(spreads)) as ArrayRef,
                Arc::new(Int64Array::from(real_vols)) as ArrayRef,
            ],
        )
        .unwrap()
    }

    fn encode_stream(batches: &[RecordBatch], schema: &Arc<Schema>) -> Vec<u8> {
        let mut buf = Vec::new();
        {
            let mut writer = StreamWriter::try_new(&mut buf, schema).unwrap();
            for batch in batches {
                writer.write(batch).unwrap();
            }
            writer.finish().unwrap();
        }
        buf
    }

    #[test]
    fn bar_field_set_matches_schema() {
        let fields = bar_field_set();
        assert_eq!(fields.len(), 8);
        assert_eq!(fields[0].name, "time");
        assert_eq!(fields[0].arrow_type, "timestamp[us]");
        assert_eq!(fields[0].tz, Some("naive-wallclock-America/Sao_Paulo"));
        assert_eq!(fields[1].name, "open");
        assert_eq!(fields[1].arrow_type, "float64");
        assert_eq!(fields[4].name, "close");
        assert_eq!(fields[7].name, "real_volume");
        assert_eq!(fields[7].arrow_type, "int64");
    }

    #[test]
    fn single_batch_round_trip() {
        let batch = make_contract_batch(1_000_000_000, 5);
        let bytes = encode_stream(&[batch], &make_contract_schema());

        let frame = decode_bar_batches(&bytes).unwrap();
        assert_eq!(frame.len(), 5);
        assert_eq!(frame.time()[0], 1_000_000_000);
        assert_eq!(frame.time()[4], 1_000_000_000 + 4 * 60_000_000);
        assert!((frame.open()[0] - 100.0).abs() < 1e-9);
        assert!((frame.close()[4] - 106.0).abs() < 1e-9);
        assert_eq!(frame.tick_volume().unwrap()[0], 10);
        assert_eq!(frame.spread().unwrap()[0], 1);
        assert_eq!(frame.real_volume().unwrap()[0], 1000);
        assert_eq!(frame.label(), TimeLabel::BrasiliaWallclock);
    }

    #[test]
    fn batches_concatenate_in_order() {
        let batch1 = make_contract_batch(1_000_000_000, 3);
        let batch2 = make_contract_batch(1_000_000_000 + 3 * 60_000_000, 2);
        let bytes = encode_stream(&[batch1, batch2], &make_contract_schema());

        let frame = decode_bar_batches(&bytes).unwrap();
        assert_eq!(frame.len(), 5);
        assert_eq!(frame.time()[2], 1_000_000_000 + 2 * 60_000_000);
        assert_eq!(frame.time()[3], 1_000_000_000 + 3 * 60_000_000);
    }

    #[test]
    fn empty_stream_gives_empty_frame() {
        let bytes = encode_stream(&[], &make_contract_schema());
        let frame = decode_bar_batches(&bytes).unwrap();
        assert_eq!(frame.len(), 0);
        assert!(frame.is_empty());
        assert_eq!(frame.schema().len(), 8);
    }

    #[test]
    fn reversed_column_order_decodes_identically() {
        let batch = make_contract_batch(1_000_000_000, 3);
        let orig_schema = make_contract_schema();
        let rev_fields: Vec<Arc<Field>> = orig_schema.fields().iter().cloned().rev().collect();
        let rev_schema = Arc::new(Schema::new(rev_fields));

        let rev_columns: Vec<ArrayRef> = batch.columns().iter().cloned().rev().collect();
        let rev_batch = RecordBatch::try_new(rev_schema.clone(), rev_columns).unwrap();

        let bytes_normal = encode_stream(&[batch], &orig_schema);
        let bytes_rev = encode_stream(&[rev_batch], &rev_schema);

        let frame_normal = decode_bar_batches(&bytes_normal).unwrap();
        let frame_rev = decode_bar_batches(&bytes_rev).unwrap();

        assert_eq!(frame_normal.time(), frame_rev.time());
        assert_eq!(frame_normal.open(), frame_rev.open());
        assert_eq!(frame_normal.high(), frame_rev.high());
        assert_eq!(frame_normal.low(), frame_rev.low());
        assert_eq!(frame_normal.close(), frame_rev.close());
        assert_eq!(frame_normal.tick_volume(), frame_rev.tick_volume());
        assert_eq!(frame_normal.spread(), frame_rev.spread());
        assert_eq!(frame_normal.real_volume(), frame_rev.real_volume());
    }

    #[test]
    fn renamed_column_gives_missing_column() {
        let fields = vec![
            Field::new(
                "time",
                DataType::Timestamp(TimeUnit::Microsecond, None),
                false,
            ),
            Field::new("open", DataType::Float64, false),
            Field::new("high", DataType::Float64, false),
            Field::new("low", DataType::Float64, false),
            Field::new("renamed_close", DataType::Float64, false),
            Field::new("tick_volume", DataType::Int64, false),
            Field::new("spread", DataType::Int64, false),
            Field::new("real_volume", DataType::Int64, false),
        ];
        let schema = Arc::new(Schema::new(fields));
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(TimestampMicrosecondArray::from(vec![1_000_000])) as ArrayRef,
                Arc::new(Float64Array::from(vec![100.0])) as ArrayRef,
                Arc::new(Float64Array::from(vec![105.0])) as ArrayRef,
                Arc::new(Float64Array::from(vec![99.0])) as ArrayRef,
                Arc::new(Float64Array::from(vec![102.0])) as ArrayRef,
                Arc::new(Int64Array::from(vec![10])) as ArrayRef,
                Arc::new(Int64Array::from(vec![1])) as ArrayRef,
                Arc::new(Int64Array::from(vec![1000])) as ArrayRef,
            ],
        )
        .unwrap();

        let bytes = encode_stream(&[batch], &schema);
        match decode_bar_batches(&bytes) {
            Err(IoError::MissingColumn { name }) => assert_eq!(name, "close"),
            other => panic!("expected MissingColumn for close, got {other:?}"),
        }
    }

    #[test]
    fn added_column_gives_unexpected_column() {
        let mut fields: Vec<Arc<Field>> = make_contract_schema().fields().iter().cloned().collect();
        fields.push(Arc::new(Field::new("extra_col", DataType::Float64, false)));
        let schema = Arc::new(Schema::new(fields));

        let base_batch = make_contract_batch(1_000_000_000, 1);
        let mut columns = base_batch.columns().to_vec();
        columns.push(Arc::new(Float64Array::from(vec![42.0])) as ArrayRef);
        let batch = RecordBatch::try_new(schema.clone(), columns).unwrap();

        let bytes = encode_stream(&[batch], &schema);
        match decode_bar_batches(&bytes) {
            Err(IoError::UnexpectedColumn { name }) => assert_eq!(name, "extra_col"),
            other => panic!("expected UnexpectedColumn for extra_col, got {other:?}"),
        }
    }

    #[test]
    fn float32_close_gives_column_type() {
        let fields = vec![
            Field::new(
                "time",
                DataType::Timestamp(TimeUnit::Microsecond, None),
                false,
            ),
            Field::new("open", DataType::Float64, false),
            Field::new("high", DataType::Float64, false),
            Field::new("low", DataType::Float64, false),
            Field::new("close", DataType::Float32, false),
            Field::new("tick_volume", DataType::Int64, false),
            Field::new("spread", DataType::Int64, false),
            Field::new("real_volume", DataType::Int64, false),
        ];
        let schema = Arc::new(Schema::new(fields));
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(TimestampMicrosecondArray::from(vec![1_000_000])) as ArrayRef,
                Arc::new(Float64Array::from(vec![100.0])) as ArrayRef,
                Arc::new(Float64Array::from(vec![105.0])) as ArrayRef,
                Arc::new(Float64Array::from(vec![99.0])) as ArrayRef,
                Arc::new(Float32Array::from(vec![102.0f32])) as ArrayRef,
                Arc::new(Int64Array::from(vec![10])) as ArrayRef,
                Arc::new(Int64Array::from(vec![1])) as ArrayRef,
                Arc::new(Int64Array::from(vec![1000])) as ArrayRef,
            ],
        )
        .unwrap();

        let bytes = encode_stream(&[batch], &schema);
        match decode_bar_batches(&bytes) {
            Err(IoError::ColumnType {
                name,
                expected,
                found,
            }) => {
                assert_eq!(name, "close");
                assert_eq!(expected, "float64");
                assert_eq!(found, "float32");
            }
            other => panic!("expected ColumnType for close, got {other:?}"),
        }
    }

    #[test]
    fn int32_time_gives_column_type() {
        let fields = vec![
            Field::new("time", DataType::Int32, false),
            Field::new("open", DataType::Float64, false),
            Field::new("high", DataType::Float64, false),
            Field::new("low", DataType::Float64, false),
            Field::new("close", DataType::Float64, false),
            Field::new("tick_volume", DataType::Int64, false),
            Field::new("spread", DataType::Int64, false),
            Field::new("real_volume", DataType::Int64, false),
        ];
        let schema = Arc::new(Schema::new(fields));
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(Int32Array::from(vec![1_000_000])) as ArrayRef,
                Arc::new(Float64Array::from(vec![100.0])) as ArrayRef,
                Arc::new(Float64Array::from(vec![105.0])) as ArrayRef,
                Arc::new(Float64Array::from(vec![99.0])) as ArrayRef,
                Arc::new(Float64Array::from(vec![102.0])) as ArrayRef,
                Arc::new(Int64Array::from(vec![10])) as ArrayRef,
                Arc::new(Int64Array::from(vec![1])) as ArrayRef,
                Arc::new(Int64Array::from(vec![1000])) as ArrayRef,
            ],
        )
        .unwrap();

        let bytes = encode_stream(&[batch], &schema);
        match decode_bar_batches(&bytes) {
            Err(IoError::ColumnType {
                name,
                expected,
                found,
            }) => {
                assert_eq!(name, "time");
                assert_eq!(expected, "timestamp[us]");
                assert_eq!(found, "int32");
            }
            other => panic!("expected ColumnType for time, got {other:?}"),
        }
    }

    #[test]
    fn null_high_gives_null_value() {
        let schema = Arc::new(Schema::new(vec![
            Field::new(
                "time",
                DataType::Timestamp(TimeUnit::Microsecond, None),
                true,
            ),
            Field::new("open", DataType::Float64, true),
            Field::new("high", DataType::Float64, true),
            Field::new("low", DataType::Float64, true),
            Field::new("close", DataType::Float64, true),
            Field::new("tick_volume", DataType::Int64, true),
            Field::new("spread", DataType::Int64, true),
            Field::new("real_volume", DataType::Int64, true),
        ]));

        let batch1 = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(TimestampMicrosecondArray::from(vec![1_000_000])) as ArrayRef,
                Arc::new(Float64Array::from(vec![100.0])) as ArrayRef,
                Arc::new(Float64Array::from(vec![105.0])) as ArrayRef,
                Arc::new(Float64Array::from(vec![99.0])) as ArrayRef,
                Arc::new(Float64Array::from(vec![102.0])) as ArrayRef,
                Arc::new(Int64Array::from(vec![10])) as ArrayRef,
                Arc::new(Int64Array::from(vec![1])) as ArrayRef,
                Arc::new(Int64Array::from(vec![1000])) as ArrayRef,
            ],
        )
        .unwrap();

        let batch2 = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(TimestampMicrosecondArray::from(vec![2_000_000])) as ArrayRef,
                Arc::new(Float64Array::from(vec![101.0])) as ArrayRef,
                Arc::new(Float64Array::from(vec![None])) as ArrayRef, // null high!
                Arc::new(Float64Array::from(vec![98.0])) as ArrayRef,
                Arc::new(Float64Array::from(vec![103.0])) as ArrayRef,
                Arc::new(Int64Array::from(vec![15])) as ArrayRef,
                Arc::new(Int64Array::from(vec![2])) as ArrayRef,
                Arc::new(Int64Array::from(vec![1500])) as ArrayRef,
            ],
        )
        .unwrap();

        let bytes = encode_stream(&[batch1, batch2], &schema);
        match decode_bar_batches(&bytes) {
            Err(IoError::NullValue { file, column, row }) => {
                assert_eq!(file, None);
                assert_eq!(column, "high");
                assert_eq!(row, 1);
            }
            other => panic!("expected NullValue for high, got {other:?}"),
        }
    }

    #[test]
    fn truncated_bytes_gives_arrow() {
        let batch = make_contract_batch(1_000_000_000, 3);
        let bytes = encode_stream(&[batch], &make_contract_schema());
        let truncated = &bytes[..bytes.len() / 2];

        match decode_bar_batches(truncated) {
            Err(IoError::Arrow { detail }) => {
                assert!(!detail.is_empty());
            }
            other => panic!("expected Arrow error, got {other:?}"),
        }
    }
}
