//! Parquet columnar reader for bar datasets.

use std::fs::File;
use std::path::Path;

use arrow::array::{
    Array, Float64Array, Int64Array, TimestampMicrosecondArray, TimestampMillisecondArray,
    TimestampNanosecondArray, TimestampSecondArray,
};
use arrow::datatypes::{DataType, TimeUnit};
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use parquet::file::reader::{FileReader, SerializedFileReader};
use q_buffers::{BarColumns, BarFrame, TimeLabel};

use crate::arrow_ipc::{bar_field_set, format_arrow_type};
use crate::error::IoError;

/// A half-open positional row range across the listed files, counted as if they
/// were concatenated. `None` means every row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RowRange {
    pub start: usize,
    pub end: usize,
}

fn check_path_pattern(path: &Path) -> Result<(), IoError> {
    let s = path.to_string_lossy();
    if s.starts_with('~')
        || s.contains('*')
        || s.contains('?')
        || s.contains('[')
        || s.contains('{')
    {
        return Err(IoError::PathPattern {
            path: s.into_owned(),
        });
    }
    Ok(())
}

/// Rows a file holds, read from its footer without decoding a row group.
pub fn bar_file_rows(path: &Path) -> Result<usize, IoError> {
    check_path_pattern(path)?;
    let file = File::open(path).map_err(|_| IoError::FileMissing {
        path: path.display().to_string(),
    })?;
    let reader = SerializedFileReader::new(file).map_err(|_| IoError::NotParquet {
        path: path.display().to_string(),
    })?;
    let total_rows = reader.metadata().file_metadata().num_rows();
    usize::try_from(total_rows).map_err(|_| IoError::Arrow {
        detail: "row count exceeds usize".to_string(),
    })
}

/// Reads the listed files, in order, into one bar frame. Opens exactly these
/// paths: no directory listing, no pattern expansion, no path derivation.
pub fn read_bar_files(paths: &[&Path], range: Option<RowRange>) -> Result<BarFrame, IoError> {
    for &path in paths {
        check_path_pattern(path)?;
    }

    let mut file_row_counts = Vec::with_capacity(paths.len());
    let mut total_rows = 0usize;
    for &path in paths {
        let count = bar_file_rows(path)?;
        file_row_counts.push(count);
        total_rows = total_rows
            .checked_add(count)
            .expect("total row count exceeds usize");
    }

    let (range_start, range_end) = match range {
        Some(r) => {
            if r.start > r.end || r.end > total_rows {
                return Err(IoError::RowRange {
                    start: r.start,
                    end: r.end,
                    rows: total_rows,
                });
            }
            (r.start, r.end)
        }
        None => (0, total_rows),
    };

    let target_len = range_end - range_start;
    if target_len == 0 {
        let bars = BarColumns {
            time: Vec::new(),
            open: Vec::new(),
            high: Vec::new(),
            low: Vec::new(),
            close: Vec::new(),
            tick_volume: Some(Vec::new()),
            spread: Some(Vec::new()),
            real_volume: Some(Vec::new()),
            label: TimeLabel::BrasiliaWallclock,
        };
        return BarFrame::try_new(bars).map_err(IoError::from);
    }

    let mut time = Vec::with_capacity(target_len);
    let mut open = Vec::with_capacity(target_len);
    let mut high = Vec::with_capacity(target_len);
    let mut low = Vec::with_capacity(target_len);
    let mut close = Vec::with_capacity(target_len);
    let mut tick_volume = Vec::with_capacity(target_len);
    let mut spread = Vec::with_capacity(target_len);
    let mut real_volume = Vec::with_capacity(target_len);

    let required_names: Vec<&str> = bar_field_set().iter().map(|f| f.name.as_str()).collect();

    let mut cur_global_row = 0usize;
    for (file_idx, &path) in paths.iter().enumerate() {
        let file_rows = file_row_counts[file_idx];
        let file_global_start = cur_global_row;
        let file_global_end = cur_global_row + file_rows;
        cur_global_row = file_global_end;

        let overlap_start = file_global_start.max(range_start);
        let overlap_end = file_global_end.min(range_end);
        if overlap_start >= overlap_end {
            continue;
        }

        let file = File::open(path).map_err(|_| IoError::FileMissing {
            path: path.display().to_string(),
        })?;
        let builder =
            ParquetRecordBatchReaderBuilder::try_new(file).map_err(|_| IoError::NotParquet {
                path: path.display().to_string(),
            })?;
        let schema = builder.schema().clone();

        // 1. Missing columns
        for &req in &required_names {
            if schema.column_with_name(req).is_none() {
                return Err(IoError::MissingColumn {
                    name: req.to_string(),
                });
            }
        }

        // 2. Unexpected columns
        for field in schema.fields() {
            if !required_names.contains(&field.name().as_str()) {
                return Err(IoError::UnexpectedColumn {
                    name: field.name().clone(),
                });
            }
        }

        // 3. Types
        for field in schema.fields() {
            let name = field.name().as_str();
            let dt = field.data_type();
            match name {
                "time" => {
                    if !matches!(
                        dt,
                        DataType::Timestamp(
                            TimeUnit::Second
                                | TimeUnit::Millisecond
                                | TimeUnit::Microsecond
                                | TimeUnit::Nanosecond,
                            _
                        )
                    ) {
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
                    if !matches!(dt, DataType::Int64 | DataType::Float64) {
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

        let time_idx = schema.index_of("time").unwrap();
        let open_idx = schema.index_of("open").unwrap();
        let high_idx = schema.index_of("high").unwrap();
        let low_idx = schema.index_of("low").unwrap();
        let close_idx = schema.index_of("close").unwrap();
        let tick_vol_idx = schema.index_of("tick_volume").unwrap();
        let spread_idx = schema.index_of("spread").unwrap();
        let real_vol_idx = schema.index_of("real_volume").unwrap();

        let reader = builder.build().map_err(|e| IoError::Arrow {
            detail: e.to_string(),
        })?;

        let mut file_cur_row = 0usize;
        for batch_res in reader {
            let batch = batch_res.map_err(|e| IoError::Arrow {
                detail: e.to_string(),
            })?;
            let batch_len = batch.num_rows();
            let batch_global_start = file_global_start + file_cur_row;
            let batch_global_end = batch_global_start + batch_len;

            let batch_overlap_start = batch_global_start.max(range_start);
            let batch_overlap_end = batch_global_end.min(range_end);

            if batch_overlap_start < batch_overlap_end {
                let local_slice_start = batch_overlap_start - batch_global_start;
                let local_slice_len = batch_overlap_end - batch_overlap_start;
                let sliced_batch = batch.slice(local_slice_start, local_slice_len);

                let time_arr = sliced_batch.column(time_idx);
                let open_arr = sliced_batch.column(open_idx);
                let high_arr = sliced_batch.column(high_idx);
                let low_arr = sliced_batch.column(low_idx);
                let close_arr = sliced_batch.column(close_idx);
                let tick_vol_arr = sliced_batch.column(tick_vol_idx);
                let spread_arr = sliced_batch.column(spread_idx);
                let real_vol_arr = sliced_batch.column(real_vol_idx);

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

                let mut earliest_null: Option<(usize, &'static str)> = None;
                for (col_name, arr) in &columns {
                    if arr.null_count() > 0 {
                        for i in 0..arr.len() {
                            if arr.is_null(i) {
                                match earliest_null {
                                    Some((min_r, _)) if i < min_r => {
                                        earliest_null = Some((i, col_name));
                                    }
                                    None => {
                                        earliest_null = Some((i, col_name));
                                    }
                                    _ => {}
                                }
                                break;
                            }
                        }
                    }
                }

                if let Some((row_in_slice, col_name)) = earliest_null {
                    return Err(IoError::NullValue {
                        file: Some(path.display().to_string()),
                        column: col_name.to_string(),
                        row: file_cur_row + local_slice_start + row_in_slice,
                    });
                }

                // Normalise and append columns
                match time_arr.data_type() {
                    DataType::Timestamp(TimeUnit::Microsecond, _) => {
                        let arr = time_arr
                            .as_any()
                            .downcast_ref::<TimestampMicrosecondArray>()
                            .unwrap();
                        time.extend_from_slice(arr.values());
                    }
                    DataType::Timestamp(TimeUnit::Second, _) => {
                        let arr = time_arr
                            .as_any()
                            .downcast_ref::<TimestampSecondArray>()
                            .unwrap();
                        time.extend(arr.values().iter().map(|&s| s * 1_000_000));
                    }
                    DataType::Timestamp(TimeUnit::Millisecond, _) => {
                        let arr = time_arr
                            .as_any()
                            .downcast_ref::<TimestampMillisecondArray>()
                            .unwrap();
                        time.extend(arr.values().iter().map(|&ms| ms * 1_000));
                    }
                    DataType::Timestamp(TimeUnit::Nanosecond, _) => {
                        let arr = time_arr
                            .as_any()
                            .downcast_ref::<TimestampNanosecondArray>()
                            .unwrap();
                        time.extend(arr.values().iter().map(|&ns| ns.div_euclid(1_000)));
                    }
                    _ => unreachable!(),
                }

                let open_prim = open_arr.as_any().downcast_ref::<Float64Array>().unwrap();
                let high_prim = high_arr.as_any().downcast_ref::<Float64Array>().unwrap();
                let low_prim = low_arr.as_any().downcast_ref::<Float64Array>().unwrap();
                let close_prim = close_arr.as_any().downcast_ref::<Float64Array>().unwrap();

                open.extend_from_slice(open_prim.values());
                high.extend_from_slice(high_prim.values());
                low.extend_from_slice(low_prim.values());
                close.extend_from_slice(close_prim.values());

                append_volume_column(tick_vol_arr, &mut tick_volume);
                append_volume_column(spread_arr, &mut spread);
                append_volume_column(real_vol_arr, &mut real_volume);
            }

            file_cur_row += batch_len;
        }
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

fn append_volume_column(arr: &arrow::array::ArrayRef, target: &mut Vec<i64>) {
    match arr.data_type() {
        DataType::Int64 => {
            let prim = arr.as_any().downcast_ref::<Int64Array>().unwrap();
            target.extend_from_slice(prim.values());
        }
        DataType::Float64 => {
            let prim = arr.as_any().downcast_ref::<Float64Array>().unwrap();
            target.extend(prim.values().iter().map(|&f| f as i64));
        }
        _ => unreachable!(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::Arc;

    use arrow::array::ArrayRef;
    use arrow::datatypes::{Field, Schema};
    use arrow::record_batch::RecordBatch;
    use parquet::arrow::ArrowWriter;
    use parquet::file::properties::WriterProperties;

    fn write_parquet_file(path: &Path, batches: &[RecordBatch]) {
        let file = File::create(path).unwrap();
        let schema = batches[0].schema();
        let props = WriterProperties::builder().build();
        let mut writer = ArrowWriter::try_new(file, schema, Some(props)).unwrap();
        for batch in batches {
            writer.write(batch).unwrap();
        }
        writer.close().unwrap();
    }

    fn make_test_batch(
        start_time: i64,
        time_unit: TimeUnit,
        tick_vol_is_float: bool,
        count: usize,
    ) -> RecordBatch {
        let time_field = Field::new("time", DataType::Timestamp(time_unit, None), false);
        let tick_vol_field = if tick_vol_is_float {
            Field::new("tick_volume", DataType::Float64, false)
        } else {
            Field::new("tick_volume", DataType::Int64, false)
        };

        let schema = Arc::new(Schema::new(vec![
            time_field,
            Field::new("open", DataType::Float64, false),
            Field::new("high", DataType::Float64, false),
            Field::new("low", DataType::Float64, false),
            Field::new("close", DataType::Float64, false),
            tick_vol_field,
            Field::new("spread", DataType::Int64, false),
            Field::new("real_volume", DataType::Int64, false),
        ]));

        let time_arr: ArrayRef = match time_unit {
            TimeUnit::Microsecond => {
                let times: Vec<i64> = (0..count)
                    .map(|i| start_time + (i as i64) * 60_000_000)
                    .collect();
                Arc::new(TimestampMicrosecondArray::from(times))
            }
            TimeUnit::Second => {
                let times: Vec<i64> = (0..count).map(|i| start_time + (i as i64) * 60).collect();
                Arc::new(TimestampSecondArray::from(times))
            }
            TimeUnit::Millisecond => {
                let times: Vec<i64> = (0..count)
                    .map(|i| start_time + (i as i64) * 60_000)
                    .collect();
                Arc::new(TimestampMillisecondArray::from(times))
            }
            TimeUnit::Nanosecond => {
                let times: Vec<i64> = (0..count)
                    .map(|i| start_time + (i as i64) * 60_000_000_000)
                    .collect();
                Arc::new(TimestampNanosecondArray::from(times))
            }
        };

        let opens: Vec<f64> = (0..count).map(|i| 100.0 + i as f64).collect();
        let highs: Vec<f64> = (0..count).map(|i| 105.0 + i as f64).collect();
        let lows: Vec<f64> = (0..count).map(|i| 99.0 + i as f64).collect();
        let closes: Vec<f64> = (0..count).map(|i| 102.0 + i as f64).collect();
        let tick_vol_arr: ArrayRef = if tick_vol_is_float {
            let vols: Vec<f64> = (0..count).map(|i| 2.9 + i as f64).collect();
            Arc::new(Float64Array::from(vols))
        } else {
            let vols: Vec<i64> = (0..count).map(|i| 10 + i as i64).collect();
            Arc::new(Int64Array::from(vols))
        };
        let spreads: Vec<i64> = (0..count).map(|i| 1 + i as i64).collect();
        let real_vols: Vec<i64> = (0..count).map(|i| 1000 + i as i64).collect();

        RecordBatch::try_new(
            schema,
            vec![
                time_arr,
                Arc::new(Float64Array::from(opens)),
                Arc::new(Float64Array::from(highs)),
                Arc::new(Float64Array::from(lows)),
                Arc::new(Float64Array::from(closes)),
                tick_vol_arr,
                Arc::new(Int64Array::from(spreads)),
                Arc::new(Int64Array::from(real_vols)),
            ],
        )
        .unwrap()
    }

    #[test]
    fn two_files_concatenate_in_listed_order() {
        let temp_dir = tempfile::tempdir().unwrap();
        let path1 = temp_dir.path().join("part1.parquet");
        let path2 = temp_dir.path().join("part2.parquet");

        let b1 = make_test_batch(1_000_000_000, TimeUnit::Microsecond, false, 3);
        let b2 = make_test_batch(
            1_000_000_000 + 3 * 60_000_000,
            TimeUnit::Microsecond,
            false,
            2,
        );

        write_parquet_file(&path1, &[b1]);
        write_parquet_file(&path2, &[b2]);

        assert_eq!(bar_file_rows(&path1).unwrap(), 3);
        assert_eq!(bar_file_rows(&path2).unwrap(), 2);

        let frame = read_bar_files(&[&path1, &path2], None).unwrap();
        assert_eq!(frame.len(), 5);
        assert_eq!(frame.time()[0], 1_000_000_000);
        assert_eq!(frame.time()[2], 1_000_000_000 + 2 * 60_000_000);
        assert_eq!(frame.time()[3], 1_000_000_000 + 3 * 60_000_000);
        assert_eq!(frame.time()[4], 1_000_000_000 + 4 * 60_000_000);
    }

    #[test]
    fn timestamp_units_floor_to_identical_microseconds() {
        let temp_dir = tempfile::tempdir().unwrap();

        // One pre-epoch value (-1_000_000_500 ns -> floors to -1_000_001 us)
        let schema_sec = Arc::new(Schema::new(vec![
            Field::new("time", DataType::Timestamp(TimeUnit::Second, None), false),
            Field::new("open", DataType::Float64, false),
            Field::new("high", DataType::Float64, false),
            Field::new("low", DataType::Float64, false),
            Field::new("close", DataType::Float64, false),
            Field::new("tick_volume", DataType::Int64, false),
            Field::new("spread", DataType::Int64, false),
            Field::new("real_volume", DataType::Int64, false),
        ]));
        let schema_ms = Arc::new(Schema::new(vec![
            Field::new(
                "time",
                DataType::Timestamp(TimeUnit::Millisecond, None),
                false,
            ),
            Field::new("open", DataType::Float64, false),
            Field::new("high", DataType::Float64, false),
            Field::new("low", DataType::Float64, false),
            Field::new("close", DataType::Float64, false),
            Field::new("tick_volume", DataType::Int64, false),
            Field::new("spread", DataType::Int64, false),
            Field::new("real_volume", DataType::Int64, false),
        ]));
        let schema_ns = Arc::new(Schema::new(vec![
            Field::new(
                "time",
                DataType::Timestamp(TimeUnit::Nanosecond, None),
                false,
            ),
            Field::new("open", DataType::Float64, false),
            Field::new("high", DataType::Float64, false),
            Field::new("low", DataType::Float64, false),
            Field::new("close", DataType::Float64, false),
            Field::new("tick_volume", DataType::Int64, false),
            Field::new("spread", DataType::Int64, false),
            Field::new("real_volume", DataType::Int64, false),
        ]));

        let dummy_prices = vec![100.0, 101.0];
        let dummy_i64 = vec![10, 10];

        // Second: -2 s -> -2_000_000 us; 5 s -> 5_000_000 us
        let b_sec = RecordBatch::try_new(
            schema_sec,
            vec![
                Arc::new(TimestampSecondArray::from(vec![-2, 5])) as ArrayRef,
                Arc::new(Float64Array::from(dummy_prices.clone())),
                Arc::new(Float64Array::from(dummy_prices.clone())),
                Arc::new(Float64Array::from(dummy_prices.clone())),
                Arc::new(Float64Array::from(dummy_prices.clone())),
                Arc::new(Int64Array::from(dummy_i64.clone())),
                Arc::new(Int64Array::from(dummy_i64.clone())),
                Arc::new(Int64Array::from(dummy_i64.clone())),
            ],
        )
        .unwrap();

        // Millisecond: -2_000 ms -> -2_000_000 us; 5_000 ms -> 5_000_000 us
        let b_ms = RecordBatch::try_new(
            schema_ms,
            vec![
                Arc::new(TimestampMillisecondArray::from(vec![-2_000, 5_000])) as ArrayRef,
                Arc::new(Float64Array::from(dummy_prices.clone())),
                Arc::new(Float64Array::from(dummy_prices.clone())),
                Arc::new(Float64Array::from(dummy_prices.clone())),
                Arc::new(Float64Array::from(dummy_prices.clone())),
                Arc::new(Int64Array::from(dummy_i64.clone())),
                Arc::new(Int64Array::from(dummy_i64.clone())),
                Arc::new(Int64Array::from(dummy_i64.clone())),
            ],
        )
        .unwrap();

        // Nanosecond: -2_000_000_500 ns (floors to -2_000_001 us), 5_000_000_800 ns (floors to 5_000_000 us)
        let b_ns = RecordBatch::try_new(
            schema_ns,
            vec![
                Arc::new(TimestampNanosecondArray::from(vec![
                    -2_000_000_500,
                    5_000_000_800,
                ])) as ArrayRef,
                Arc::new(Float64Array::from(dummy_prices.clone())),
                Arc::new(Float64Array::from(dummy_prices.clone())),
                Arc::new(Float64Array::from(dummy_prices.clone())),
                Arc::new(Float64Array::from(dummy_prices)),
                Arc::new(Int64Array::from(dummy_i64.clone())),
                Arc::new(Int64Array::from(dummy_i64.clone())),
                Arc::new(Int64Array::from(dummy_i64)),
            ],
        )
        .unwrap();

        let path_sec = temp_dir.path().join("sec.parquet");
        let path_ms = temp_dir.path().join("ms.parquet");
        let path_ns = temp_dir.path().join("ns.parquet");

        write_parquet_file(&path_sec, &[b_sec]);
        write_parquet_file(&path_ms, &[b_ms]);
        write_parquet_file(&path_ns, &[b_ns]);

        let frame_sec = read_bar_files(&[&path_sec], None).unwrap();
        assert_eq!(frame_sec.time(), &[-2_000_000, 5_000_000]);

        let frame_ms = read_bar_files(&[&path_ms], None).unwrap();
        assert_eq!(frame_ms.time(), &[-2_000_000, 5_000_000]);

        let frame_ns = read_bar_files(&[&path_ns], None).unwrap();
        assert_eq!(frame_ns.time(), &[-2_000_001, 5_000_000]);
    }

    #[test]
    fn float64_volume_truncates_toward_zero() {
        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().join("float_vol.parquet");

        let schema = Arc::new(Schema::new(vec![
            Field::new(
                "time",
                DataType::Timestamp(TimeUnit::Microsecond, None),
                false,
            ),
            Field::new("open", DataType::Float64, false),
            Field::new("high", DataType::Float64, false),
            Field::new("low", DataType::Float64, false),
            Field::new("close", DataType::Float64, false),
            Field::new("tick_volume", DataType::Float64, false),
            Field::new("spread", DataType::Float64, false),
            Field::new("real_volume", DataType::Float64, false),
        ]));

        let batch = RecordBatch::try_new(
            schema,
            vec![
                Arc::new(TimestampMicrosecondArray::from(vec![1_000_000, 2_000_000])) as ArrayRef,
                Arc::new(Float64Array::from(vec![100.0, 101.0])),
                Arc::new(Float64Array::from(vec![105.0, 106.0])),
                Arc::new(Float64Array::from(vec![99.0, 100.0])),
                Arc::new(Float64Array::from(vec![102.0, 103.0])),
                Arc::new(Float64Array::from(vec![2.9, -2.9])),
                Arc::new(Float64Array::from(vec![1.1, -1.9])),
                Arc::new(Float64Array::from(vec![1000.8, -1000.2])),
            ],
        )
        .unwrap();

        write_parquet_file(&path, &[batch]);

        let frame = read_bar_files(&[&path], None).unwrap();
        assert_eq!(frame.tick_volume().unwrap(), &[2, -2]);
        assert_eq!(frame.spread().unwrap(), &[1, -1]);
        assert_eq!(frame.real_volume().unwrap(), &[1000, -1000]);
    }

    #[test]
    fn null_open_gives_null_value() {
        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().join("null_open.parquet");

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

        let batch = RecordBatch::try_new(
            schema,
            vec![
                Arc::new(TimestampMicrosecondArray::from(vec![1_000_000, 2_000_000])) as ArrayRef,
                Arc::new(Float64Array::from(vec![Some(100.0), None])), // null open at row 1!
                Arc::new(Float64Array::from(vec![105.0, 106.0])),
                Arc::new(Float64Array::from(vec![99.0, 100.0])),
                Arc::new(Float64Array::from(vec![102.0, 103.0])),
                Arc::new(Int64Array::from(vec![10, 10])),
                Arc::new(Int64Array::from(vec![1, 1])),
                Arc::new(Int64Array::from(vec![1000, 1000])),
            ],
        )
        .unwrap();

        write_parquet_file(&path, &[batch]);

        match read_bar_files(&[&path], None) {
            Err(IoError::NullValue { file, column, row }) => {
                assert_eq!(file, Some(path.display().to_string()));
                assert_eq!(column, "open");
                assert_eq!(row, 1);
            }
            other => panic!("expected NullValue for open at row 1, got {other:?}"),
        }
    }

    #[test]
    fn missing_file_gives_file_missing() {
        let missing = Path::new("/nonexistent/bars.parquet");
        match read_bar_files(&[missing], None) {
            Err(IoError::FileMissing { path }) => {
                assert!(path.contains("nonexistent"));
            }
            other => panic!("expected FileMissing, got {other:?}"),
        }
    }

    #[test]
    fn text_file_gives_not_parquet() {
        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().join("text.txt");
        let mut file = File::create(&path).unwrap();
        file.write_all(b"this is not parquet data").unwrap();
        drop(file);

        match read_bar_files(&[&path], None) {
            Err(IoError::NotParquet { path: p }) => {
                assert_eq!(p, path.display().to_string());
            }
            other => panic!("expected NotParquet, got {other:?}"),
        }
    }

    #[test]
    fn glob_path_gives_path_pattern() {
        let p1 = Path::new("data/bars-*.parquet");
        let p2 = Path::new("~/bars.parquet");
        let p3 = Path::new("data/bars-?.parquet");
        let p4 = Path::new("data/bars-[0-9].parquet");
        let p5 = Path::new("data/bars-{a,b}.parquet");

        for p in [p1, p2, p3, p4, p5] {
            match read_bar_files(&[p], None) {
                Err(IoError::PathPattern { path }) => {
                    assert_eq!(path, p.to_string_lossy());
                }
                other => panic!("expected PathPattern for {p:?}, got {other:?}"),
            }
        }
    }

    #[test]
    fn row_range_spanning_file_boundary_and_past_end() {
        let temp_dir = tempfile::tempdir().unwrap();
        let path1 = temp_dir.path().join("p1.parquet");
        let path2 = temp_dir.path().join("p2.parquet");

        let b1 = make_test_batch(1_000_000_000, TimeUnit::Microsecond, false, 10);
        let b2 = make_test_batch(
            1_000_000_000 + 10 * 60_000_000,
            TimeUnit::Microsecond,
            false,
            10,
        );

        write_parquet_file(&path1, &[b1]);
        write_parquet_file(&path2, &[b2]);

        // Range [7, 13) spans boundary: 3 rows from p1 (7,8,9), 3 rows from p2 (10,11,12)
        let range = RowRange { start: 7, end: 13 };
        let frame = read_bar_files(&[&path1, &path2], Some(range)).unwrap();
        assert_eq!(frame.len(), 6);
        assert_eq!(frame.time()[0], 1_000_000_000 + 7 * 60_000_000);
        assert_eq!(frame.time()[2], 1_000_000_000 + 9 * 60_000_000);
        assert_eq!(frame.time()[3], 1_000_000_000 + 10 * 60_000_000);
        assert_eq!(frame.time()[5], 1_000_000_000 + 12 * 60_000_000);

        // Range past end
        let past_end = RowRange { start: 5, end: 25 };
        match read_bar_files(&[&path1, &path2], Some(past_end)) {
            Err(IoError::RowRange { start, end, rows }) => {
                assert_eq!(start, 5);
                assert_eq!(end, 25);
                assert_eq!(rows, 20);
            }
            other => panic!("expected RowRange, got {other:?}"),
        }
    }
}
