use std::alloc::{GlobalAlloc, Layout, System};
use std::fs::File;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use arrow::array::{ArrayRef, Float64Array, Int64Array, TimestampMicrosecondArray};
use arrow::datatypes::{DataType, Field, Schema, TimeUnit};
use arrow::record_batch::RecordBatch;
use parquet::arrow::ArrowWriter;
use parquet::file::properties::WriterProperties;
use q_io::read_bar_files;

struct TrackingAllocator;

static CURRENT_ALLOCATED: AtomicUsize = AtomicUsize::new(0);
static PEAK_ALLOCATED: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for TrackingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            let current =
                CURRENT_ALLOCATED.fetch_add(layout.size(), Ordering::SeqCst) + layout.size();
            PEAK_ALLOCATED.fetch_max(current, Ordering::SeqCst);
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) };
        CURRENT_ALLOCATED.fetch_sub(layout.size(), Ordering::SeqCst);
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let new_ptr = unsafe { System.realloc(ptr, layout, new_size) };
        if !new_ptr.is_null() {
            if new_size > layout.size() {
                let diff = new_size - layout.size();
                let current = CURRENT_ALLOCATED.fetch_add(diff, Ordering::SeqCst) + diff;
                PEAK_ALLOCATED.fetch_max(current, Ordering::SeqCst);
            } else if new_size < layout.size() {
                let diff = layout.size() - new_size;
                CURRENT_ALLOCATED.fetch_sub(diff, Ordering::SeqCst);
            }
        }
        new_ptr
    }
}

#[global_allocator]
static ALLOCATOR: TrackingAllocator = TrackingAllocator;

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
    let opens: Vec<f64> = (0..count).map(|i| 100.0 + i as f64).collect();
    let highs: Vec<f64> = (0..count).map(|i| 105.0 + i as f64).collect();
    let lows: Vec<f64> = (0..count).map(|i| 99.0 + i as f64).collect();
    let closes: Vec<f64> = (0..count).map(|i| 102.0 + i as f64).collect();
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

fn write_eight_row_group_file(path: &Path, rows_per_group: usize) {
    let file = File::create(path).unwrap();
    let schema = make_schema();
    let props = WriterProperties::builder()
        .set_max_row_group_row_count(Some(rows_per_group))
        .build();
    let mut writer = ArrowWriter::try_new(file, schema, Some(props)).unwrap();

    for rg in 0..8 {
        let start_time = 1_000_000_000 + (rg * rows_per_group as i64) * 60_000_000;
        let batch = make_batch(start_time, rows_per_group);
        writer.write(&batch).unwrap();
    }
    writer.close().unwrap();
}

#[test]
fn peak_memory_beyond_frame_bounded_to_under_two_row_groups() {
    let temp_dir = tempfile::tempdir().unwrap();
    let path = temp_dir.path().join("eight_groups.parquet");
    let rows_per_group = 10_000;
    write_eight_row_group_file(&path, rows_per_group);

    // Warm-up or establish baseline before read
    let base_allocated = CURRENT_ALLOCATED.load(Ordering::SeqCst);
    PEAK_ALLOCATED.store(base_allocated, Ordering::SeqCst);

    let frame = read_bar_files(&[&path], None).unwrap();
    assert_eq!(frame.len(), 8 * rows_per_group);

    let peak = PEAK_ALLOCATED.load(Ordering::SeqCst);
    let final_allocated = CURRENT_ALLOCATED.load(Ordering::SeqCst);

    // One row group in uncompressed memory: 8 columns * 8 bytes * rows_per_group
    let row_group_uncompressed_bytes = 8 * 8 * rows_per_group;
    let two_row_groups_bytes = 2 * row_group_uncompressed_bytes;

    // Peak bytes beyond the resulting frame:
    // Memory reached at peak minus final memory (which holds the frame + base)
    let peak_beyond_final = peak.saturating_sub(final_allocated);
    let ratio = (peak_beyond_final as f64) / (row_group_uncompressed_bytes as f64);
    println!(
        "Peak beyond final frame: {} bytes (ratio: {:.3} row groups, limit: 2.0)",
        peak_beyond_final, ratio
    );

    assert!(
        peak_beyond_final < two_row_groups_bytes,
        "peak memory beyond frame was {} bytes ({:.3} row groups), expected < {} bytes (2 row groups)",
        peak_beyond_final,
        ratio,
        two_row_groups_bytes
    );
}
