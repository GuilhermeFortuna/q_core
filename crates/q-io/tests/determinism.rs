use std::fs::File;
use std::path::Path;
use std::sync::Arc;

use arrow::array::{ArrayRef, Float64Array, Int64Array, TimestampMicrosecondArray};
use arrow::datatypes::{DataType, Field, Schema, TimeUnit};
use arrow::ipc::writer::StreamWriter;
use arrow::record_batch::RecordBatch;
use parquet::arrow::ArrowWriter;
use parquet::file::properties::WriterProperties;
use q_buffers::BarFrame;
use q_io::{decode_bar_batches, read_bar_files, RowRange};

fn make_schema() -> Arc<Schema> {
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

fn make_batch(start_time: i64, count: usize) -> RecordBatch {
    let times: Vec<i64> = (0..count)
        .map(|i| start_time + (i as i64) * 60_000_000)
        .collect();
    let opens: Vec<f64> = (0..count).map(|i| 100.0 + (i as f64)).collect();
    let highs: Vec<f64> = (0..count).map(|i| 105.0 + (i as f64)).collect();
    let lows: Vec<f64> = (0..count).map(|i| 99.0 + (i as f64)).collect();
    let closes: Vec<f64> = (0..count).map(|i| 102.0 + (i as f64)).collect();
    let tick_vols: Vec<i64> = (0..count).map(|i| 10 + i as i64).collect();
    let spreads: Vec<i64> = (0..count).map(|i| 1 + i as i64).collect();
    let real_vols: Vec<i64> = (0..count).map(|i| 1000 + i as i64).collect();

    RecordBatch::try_new(
        make_schema(),
        vec![
            Arc::new(
                TimestampMicrosecondArray::from(times)
                    .with_timezone("naive-wallclock-America/Sao_Paulo"),
            ) as ArrayRef,
            Arc::new(Float64Array::from(opens)),
            Arc::new(Float64Array::from(highs)),
            Arc::new(Float64Array::from(lows)),
            Arc::new(Float64Array::from(closes)),
            Arc::new(Int64Array::from(tick_vols)),
            Arc::new(Int64Array::from(spreads)),
            Arc::new(Int64Array::from(real_vols)),
        ],
    )
    .unwrap()
}

fn assert_frames_bit_identical(f1: &BarFrame, f2: &BarFrame) {
    assert_eq!(f1.len(), f2.len());
    assert_eq!(f1.label(), f2.label());
    assert_eq!(f1.time(), f2.time());

    assert_eq!(f1.open().len(), f2.open().len());
    for (i, (&a, &b)) in f1.open().iter().zip(f2.open().iter()).enumerate() {
        assert_eq!(
            a.to_bits(),
            b.to_bits(),
            "open mismatch at index {i}: {a} vs {b}"
        );
    }

    assert_eq!(f1.high().len(), f2.high().len());
    for (i, (&a, &b)) in f1.high().iter().zip(f2.high().iter()).enumerate() {
        assert_eq!(
            a.to_bits(),
            b.to_bits(),
            "high mismatch at index {i}: {a} vs {b}"
        );
    }

    assert_eq!(f1.low().len(), f2.low().len());
    for (i, (&a, &b)) in f1.low().iter().zip(f2.low().iter()).enumerate() {
        assert_eq!(
            a.to_bits(),
            b.to_bits(),
            "low mismatch at index {i}: {a} vs {b}"
        );
    }

    assert_eq!(f1.close().len(), f2.close().len());
    for (i, (&a, &b)) in f1.close().iter().zip(f2.close().iter()).enumerate() {
        assert_eq!(
            a.to_bits(),
            b.to_bits(),
            "close mismatch at index {i}: {a} vs {b}"
        );
    }

    assert_eq!(f1.tick_volume(), f2.tick_volume());
    assert_eq!(f1.spread(), f2.spread());
    assert_eq!(f1.real_volume(), f2.real_volume());
}

fn write_parquet(path: &Path, batches: &[RecordBatch]) {
    let file = File::create(path).unwrap();
    let schema = batches[0].schema();
    let props = WriterProperties::builder().build();
    let mut writer = ArrowWriter::try_new(file, schema, Some(props)).unwrap();
    for batch in batches {
        writer.write(batch).unwrap();
    }
    writer.close().unwrap();
}

#[test]
fn arrow_ipc_decoding_twice_gives_bit_identical_frames() {
    let batch1 = make_batch(1_000_000_000, 50);
    let batch2 = make_batch(1_000_000_000 + 50 * 60_000_000, 50);

    let mut bytes = Vec::new();
    {
        let schema = make_schema();
        let mut writer = StreamWriter::try_new(&mut bytes, &schema).unwrap();
        writer.write(&batch1).unwrap();
        writer.write(&batch2).unwrap();
        writer.finish().unwrap();
    }

    let f1 = decode_bar_batches(&bytes).unwrap();
    let f2 = decode_bar_batches(&bytes).unwrap();

    assert_frames_bit_identical(&f1, &f2);
}

#[test]
fn parquet_reading_twice_gives_bit_identical_frames() {
    let temp_dir = tempfile::tempdir().unwrap();
    let path1 = temp_dir.path().join("part1.parquet");
    let path2 = temp_dir.path().join("part2.parquet");

    let b1 = make_batch(1_000_000_000, 40);
    let b2 = make_batch(1_000_000_000 + 40 * 60_000_000, 40);

    write_parquet(&path1, &[b1]);
    write_parquet(&path2, &[b2]);

    let f1 = read_bar_files(&[&path1, &path2], None).unwrap();
    let f2 = read_bar_files(&[&path1, &path2], None).unwrap();
    assert_frames_bit_identical(&f1, &f2);

    let range = RowRange { start: 15, end: 65 };
    let r1 = read_bar_files(&[&path1, &path2], Some(range)).unwrap();
    let r2 = read_bar_files(&[&path1, &path2], Some(range)).unwrap();
    assert_frames_bit_identical(&r1, &r2);
}
