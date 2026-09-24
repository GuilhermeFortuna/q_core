#![allow(
    clippy::disallowed_types,
    clippy::disallowed_methods,
    clippy::float_cmp,
    clippy::suboptimal_flops
)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Instant;

use q_buffers::geometry::{pack, pack_bucket, Surface, Viewport};
use q_buffers::lod::reduce_into;
use q_buffers::lod::Bucket;
use q_buffers::series::LiveBarSeries;
use q_buffers::{BarColumns, TimeLabel, VolumeSet};

struct CountingAllocator;
static ALLOC_COUNT: AtomicUsize = AtomicUsize::new(0);
static TRACKING_ENABLED: AtomicBool = AtomicBool::new(false);

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if TRACKING_ENABLED.load(Ordering::Relaxed) {
            ALLOC_COUNT.fetch_add(1, Ordering::Relaxed);
        }
        System.alloc(layout)
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        System.dealloc(ptr, layout)
    }
}

#[global_allocator]
static A: CountingAllocator = CountingAllocator;

fn main() {
    println!("=== Bar Geometry Benchmark ===");
    println!("Generating 500,000 bars...");

    let n = 500_000usize;
    let mut time = Vec::with_capacity(n);
    let mut open = Vec::with_capacity(n);
    let mut high = Vec::with_capacity(n);
    let mut low = Vec::with_capacity(n);
    let mut close = Vec::with_capacity(n);

    for i in 0..n {
        let t = 1_000_000_000i64 + (i as i64) * 60;
        let o = 100.0 + (i as f64) * 0.001;
        let h = o + 1.0;
        let l = o - 0.8;
        let c = if i % 2 == 0 { o + 0.5 } else { o - 0.3 };
        time.push(t);
        open.push(o);
        high.push(h);
        low.push(l);
        close.push(c);
    }

    let bars = BarColumns {
        time,
        open,
        high,
        low,
        close,
        tick_volume: None,
        spread: None,
        real_volume: None,
        label: TimeLabel::Utc,
    };

    let vols = VolumeSet {
        tick_volume: false,
        spread: false,
        real_volume: false,
    };

    let mut series = LiveBarSeries::new(n, vols, TimeLabel::Utc).expect("valid capacity");
    series.load_history(bars).expect("history load");

    let extents = series.extents().expect("extents");
    let view = Viewport {
        first_bar: 0,
        last_bar: n,
        low: extents.low,
        high: extents.high,
    };

    let bucket_configs = [500usize, 2000usize, 8000usize];
    let runs = 5;

    for &num_buckets in &bucket_configs {
        println!();
        println!(
            "--- Viewport: {} buckets (over {} bars) ---",
            num_buckets, n
        );
        let surface = Surface {
            width_px: num_buckets as f32,
            height_px: 1000.0,
        };

        let mut buckets = Vec::new();
        let mut vertices = Vec::new();

        // Call 1: warm up and pre-size buffers
        TRACKING_ENABLED.store(false, Ordering::SeqCst);
        let _ = reduce_into(series.completed(), 0..n, num_buckets, &mut buckets);
        pack(&buckets, view, surface, &mut vertices);

        let mut run_times = Vec::with_capacity(runs);
        let mut alloc_counts = Vec::with_capacity(runs);

        for run in 1..=runs {
            ALLOC_COUNT.store(0, Ordering::SeqCst);
            TRACKING_ENABLED.store(true, Ordering::SeqCst);

            let t0 = Instant::now();
            let _ = reduce_into(series.completed(), 0..n, num_buckets, &mut buckets);
            pack(&buckets, view, surface, &mut vertices);
            let elapsed = t0.elapsed();

            TRACKING_ENABLED.store(false, Ordering::SeqCst);
            let allocs = ALLOC_COUNT.load(Ordering::SeqCst);

            let elapsed_ms = elapsed.as_secs_f64() * 1000.0;
            println!(
                "  Run {}: {:.4} ms ({} allocs, {} vertices)",
                run,
                elapsed_ms,
                allocs,
                vertices.len()
            );
            run_times.push(elapsed_ms);
            alloc_counts.push(allocs);
        }

        let mut sorted = run_times.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let median_ms = sorted[runs / 2];
        let total_post_allocs: usize = alloc_counts.iter().sum();

        println!("  Median time: {:.4} ms", median_ms);
        println!("  Allocations after first call: {}", total_post_allocs);

        assert_eq!(
            total_post_allocs, 0,
            "must allocate nothing after first call"
        );
    }

    println!();
    println!("=== Split forming tick benchmark ===");

    let forming_view = Viewport {
        first_bar: 0,
        last_bar: n + 1,
        low: extents.low,
        high: extents.high,
    };

    for &num_buckets in &bucket_configs {
        println!();
        println!(
            "--- Split path: {} visible buckets (forming tick over {} bars) ---",
            num_buckets, n
        );
        let surface = Surface {
            width_px: num_buckets as f32,
            height_px: 1000.0,
        };

        let mut completed_buckets = Vec::new();
        let mut completed_vertices = Vec::new();
        let mut forming_vertices = Vec::new();
        let mut combined_buckets = Vec::new();
        let mut combined_vertices = Vec::new();
        let completed = series.completed();
        let mut forming_close = completed.close()[n - 1];

        let mut forming_bucket = Bucket {
            start: n,
            end: n + 1,
            open: completed.open()[n - 1],
            high: completed.high()[n - 1] + 1.0,
            low: completed.low()[n - 1] - 1.0,
            close: forming_close,
            forming: true,
        };

        TRACKING_ENABLED.store(false, Ordering::SeqCst);
        let _ = reduce_into(
            series.completed(),
            0..n,
            num_buckets,
            &mut completed_buckets,
        );
        let completed_count = completed_buckets.len();
        completed_vertices.clear();
        for (i, bucket) in completed_buckets.iter().enumerate() {
            pack_bucket(
                bucket,
                i,
                completed_count,
                forming_view,
                surface,
                &mut completed_vertices,
            );
        }
        pack_bucket(
            &forming_bucket,
            0,
            1,
            forming_view,
            surface,
            &mut forming_vertices,
        );

        let _ = reduce_into(series.completed(), 0..n, num_buckets, &mut combined_buckets);
        combined_buckets.push(forming_bucket);
        pack(
            &combined_buckets,
            forming_view,
            surface,
            &mut combined_vertices,
        );

        let mut combined_times = Vec::with_capacity(runs);
        let mut split_times = Vec::with_capacity(runs);
        let mut combined_allocs = Vec::with_capacity(runs);
        let mut split_allocs = Vec::with_capacity(runs);

        for run in 1..=runs {
            forming_close += 0.01;
            forming_bucket.close = forming_close;
            forming_bucket.high = forming_close + 1.0;

            let completed_snapshot_len = completed_vertices.len();
            let completed_snapshot_tail = completed_vertices.last().map(|v| {
                (
                    v.x.to_bits(),
                    v.y.to_bits(),
                    v.direction.to_bits(),
                    v.forming.to_bits(),
                )
            });

            ALLOC_COUNT.store(0, Ordering::SeqCst);
            TRACKING_ENABLED.store(true, Ordering::SeqCst);
            let t0 = Instant::now();
            forming_vertices.clear();
            pack_bucket(
                &forming_bucket,
                0,
                1,
                forming_view,
                surface,
                &mut forming_vertices,
            );
            let split_elapsed = t0.elapsed();
            TRACKING_ENABLED.store(false, Ordering::SeqCst);
            let split_alloc = ALLOC_COUNT.load(Ordering::SeqCst);

            assert_eq!(completed_vertices.len(), completed_snapshot_len);
            if let Some(tail) = completed_snapshot_tail {
                let last = completed_vertices.last().expect("completed tail");
                assert_eq!(last.x.to_bits(), tail.0);
                assert_eq!(last.y.to_bits(), tail.1);
                assert_eq!(last.direction.to_bits(), tail.2);
                assert_eq!(last.forming.to_bits(), tail.3);
            }

            ALLOC_COUNT.store(0, Ordering::SeqCst);
            TRACKING_ENABLED.store(true, Ordering::SeqCst);
            let t0 = Instant::now();
            let _ = reduce_into(series.completed(), 0..n, num_buckets, &mut combined_buckets);
            combined_buckets.push(forming_bucket);
            pack(
                &combined_buckets,
                forming_view,
                surface,
                &mut combined_vertices,
            );
            let combined_elapsed = t0.elapsed();
            TRACKING_ENABLED.store(false, Ordering::SeqCst);
            let combined_alloc = ALLOC_COUNT.load(Ordering::SeqCst);

            let split_ms = split_elapsed.as_secs_f64() * 1000.0;
            let combined_ms = combined_elapsed.as_secs_f64() * 1000.0;
            println!(
                "  Run {}: combined {:.4} ms ({} allocs), split {:.4} ms ({} allocs)",
                run, combined_ms, combined_alloc, split_ms, split_alloc
            );
            combined_times.push(combined_ms);
            split_times.push(split_ms);
            combined_allocs.push(combined_alloc);
            split_allocs.push(split_alloc);
        }

        let mut combined_sorted = combined_times.clone();
        combined_sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let mut split_sorted = split_times.clone();
        split_sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
        println!(
            "  Median combined: {:.4} ms, split: {:.4} ms",
            combined_sorted[runs / 2],
            split_sorted[runs / 2]
        );
        println!(
            "  Allocations after warmup: combined {}, split {}",
            combined_allocs.iter().sum::<usize>(),
            split_allocs.iter().sum::<usize>()
        );
        assert_eq!(split_allocs.iter().sum::<usize>(), 0);
    }

    println!();
    println!("Benchmark completed successfully.");
}
