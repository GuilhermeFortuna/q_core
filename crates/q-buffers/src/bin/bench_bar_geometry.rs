#![allow(
    clippy::disallowed_types,
    clippy::disallowed_methods,
    clippy::float_cmp,
    clippy::suboptimal_flops
)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Instant;

use q_buffers::geometry::{pack, Surface, Viewport};
use q_buffers::lod::reduce_into;
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
    println!("Benchmark completed successfully.");
}
