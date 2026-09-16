#![allow(
    clippy::disallowed_types,
    clippy::disallowed_methods,
    clippy::float_cmp,
    clippy::suboptimal_flops
)]

use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use arrow::array::{ArrayRef, Float64Array, Int64Array, TimestampMicrosecondArray};
use arrow::datatypes::{DataType, Field, Schema, TimeUnit};
use arrow::record_batch::RecordBatch;
use parquet::arrow::ArrowWriter;
use parquet::file::properties::WriterProperties;
use q_io::{bar_file_rows, read_bar_files};

fn get_peak_rss_kb() -> usize {
    let mut rusage = std::mem::MaybeUninit::<libc::rusage>::uninit();
    let ret = unsafe { libc::getrusage(libc::RUSAGE_SELF, rusage.as_mut_ptr()) };
    if ret == 0 {
        let rusage = unsafe { rusage.assume_init() };
        rusage.ru_maxrss as usize
    } else {
        0
    }
}

fn make_synthetic_month_file(path: &Path) {
    let schema = Arc::new(Schema::new(vec![
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
    ]));

    let count = 20 * 540; // 20 trading days of 540 M1 bars = 10,800 bars
    let times: Vec<i64> = (0..count)
        .map(|i| 1_700_000_000_000_000 + (i as i64) * 60_000_000)
        .collect();
    let opens: Vec<f64> = (0..count).map(|i| 120_000.0 + (i as f64) * 0.5).collect();
    let highs: Vec<f64> = (0..count).map(|i| 120_050.0 + (i as f64) * 0.5).collect();
    let lows: Vec<f64> = (0..count).map(|i| 119_950.0 + (i as f64) * 0.5).collect();
    let closes: Vec<f64> = (0..count).map(|i| 120_020.0 + (i as f64) * 0.5).collect();
    let tick_vols: Vec<i64> = (0..count).map(|i| 500 + (i as i64) % 100).collect();
    let spreads: Vec<i64> = (0..count).map(|i| 1 + (i as i64) % 5).collect();
    let real_vols: Vec<i64> = (0..count).map(|i| 5000 + (i as i64) % 1000).collect();

    let batch = RecordBatch::try_new(
        schema.clone(),
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
    .unwrap();

    let file = File::create(path).unwrap();
    let props = WriterProperties::builder().build();
    let mut writer = ArrowWriter::try_new(file, schema, Some(props)).unwrap();
    writer.write(&batch).unwrap();
    writer.close().unwrap();
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mut dump_json_path: Option<PathBuf> = None;
    let mut positional_paths: Vec<PathBuf> = Vec::new();

    let mut i = 1;
    while i < args.len() {
        if args[i] == "--dump-json" {
            i += 1;
            if i < args.len() {
                dump_json_path = Some(PathBuf::from(&args[i]));
            }
        } else if !args[i].starts_with("--") {
            positional_paths.push(PathBuf::from(&args[i]));
        }
        i += 1;
    }

    let mut target_paths = positional_paths;
    if target_paths.is_empty() {
        if let Ok(val) = std::env::var("Q_LAKE_RANGE") {
            let val = val.trim();
            if !val.is_empty() {
                for part in val.split([',', ':']) {
                    let trimmed = part.trim();
                    if !trimmed.is_empty() {
                        target_paths.push(PathBuf::from(trimmed));
                    }
                }
            }
        }
    }

    if target_paths.is_empty() {
        // Look for existing fixtures
        let candidates = [
            "contracts/tests/fixtures/bars_sample.parquet",
            "../q_contracts/tests/fixtures/bars_sample.parquet",
            "../q_backend/tests/fixtures/lake/ohlcv/WIN$N/M15/2025.parquet",
        ];
        let mut found = None;
        for c in candidates {
            let p = Path::new(c);
            if p.exists() {
                found = Some(p.to_path_buf());
                break;
            }
        }

        if let Some(p) = found {
            target_paths.push(p);
        } else {
            let synth_path = std::env::temp_dir()
                .join(format!("synthetic_month_m1_{}.parquet", std::process::id()));
            make_synthetic_month_file(&synth_path);
            target_paths.push(synth_path);
        }
    }

    println!("=== Parquet Read Benchmark ===");
    println!("Target files ({}):", target_paths.len());
    let mut total_expected_rows = 0usize;
    for p in &target_paths {
        let rows = bar_file_rows(p).expect("failed to inspect parquet rows");
        println!("  {} ({} rows)", p.display(), rows);
        total_expected_rows += rows;
    }
    println!("Total rows per read: {}", total_expected_rows);
    println!();

    let path_refs: Vec<&Path> = target_paths.iter().map(|p| p.as_path()).collect();

    let runs = 5;
    let mut wall_times = Vec::with_capacity(runs);
    let mut bars_per_sec = Vec::with_capacity(runs);

    let mut last_frame = None;
    for run in 1..=runs {
        let t0 = Instant::now();
        let frame = read_bar_files(&path_refs, None).expect("failed to read bar files");
        let elapsed = t0.elapsed().as_secs_f64();
        let bps = (frame.len() as f64) / elapsed;
        println!("Run {}: {:.4} s ({:.0} bars/s)", run, elapsed, bps);
        wall_times.push(elapsed);
        bars_per_sec.push(bps);
        last_frame = Some(frame);
    }

    let mut sorted_times = wall_times.clone();
    sorted_times.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let mut sorted_bps = bars_per_sec.clone();
    sorted_bps.sort_by(|a, b| a.partial_cmp(b).unwrap());

    let median_wall = sorted_times[runs / 2];
    let median_bps = sorted_bps[runs / 2];
    let peak_rss_kb = get_peak_rss_kb();
    let peak_rss_mb = (peak_rss_kb as f64) / 1024.0;

    println!();
    println!("Median wall time: {:.4} s", median_wall);
    println!("Median throughput: {:.0} bars/s", median_bps);
    println!("Peak RSS: {:.2} MB ({} KB)", peak_rss_mb, peak_rss_kb);

    if let (Some(out_path), Some(frame)) = (dump_json_path, last_frame) {
        let mut file = File::create(&out_path).expect("failed to create dump JSON");
        let path_strings: Vec<String> = target_paths
            .iter()
            .map(|p| p.display().to_string())
            .collect();
        write!(
            file,
            "{{\"paths\":{:?},\"rows\":{},\"time\":{:?},\"open\":{:?},\"high\":{:?},\"low\":{:?},\"close\":{:?},\"tick_volume\":{:?},\"spread\":{:?},\"real_volume\":{:?}}}",
            path_strings,
            frame.len(),
            frame.time(),
            frame.open(),
            frame.high(),
            frame.low(),
            frame.close(),
            frame.tick_volume().unwrap_or(&[]),
            frame.spread().unwrap_or(&[]),
            frame.real_volume().unwrap_or(&[])
        )
        .expect("failed to write dump JSON");
        println!();
        println!("Exported decoded rows to {}", out_path.display());
    }
}
