//! Level of detail: exact deterministic bucket reduction over columnar bar frames.

use std::ops::Range;

use crate::frame::BarFrame;
use crate::series::SeriesError;

/// One downsampled or single-bar bucket.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bucket {
    /// Half-open bar index range `[start, end)`.
    pub start: usize,
    pub end: usize,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
}

/// Exact reduction of `range` to at most `columns` buckets.
pub fn reduce(
    frame: &BarFrame,
    range: Range<usize>,
    columns: usize,
) -> Result<Vec<Bucket>, SeriesError> {
    let mut out = Vec::new();
    reduce_into(frame, range, columns, &mut out)?;
    Ok(out)
}

/// Reduces into a caller-owned buffer, reusing its capacity to eliminate allocations.
pub fn reduce_into(
    frame: &BarFrame,
    range: Range<usize>,
    columns: usize,
    out: &mut Vec<Bucket>,
) -> Result<(), SeriesError> {
    if columns == 0 {
        return Err(SeriesError::Empty);
    }

    out.clear();

    let frame_len = frame.len();
    let start = range.start.min(frame_len);
    let end = range.end.min(frame_len);
    if start >= end {
        return Ok(());
    }

    let n = end - start;
    if n <= columns {
        // Range no longer than column count: each bar is its own bucket.
        out.reserve(n);
        let opens = frame.open();
        let highs = frame.high();
        let lows = frame.low();
        let closes = frame.close();

        for i in 0..n {
            let bar_idx = start + i;
            out.push(Bucket {
                start: bar_idx,
                end: bar_idx + 1,
                open: opens[bar_idx],
                high: highs[bar_idx],
                low: lows[bar_idx],
                close: closes[bar_idx],
            });
        }
        return Ok(());
    }

    // Exact reduction using integer boundaries derived only from range and columns.
    out.reserve(columns);
    let opens = frame.open();
    let highs = frame.high();
    let lows = frame.low();
    let closes = frame.close();

    for col in 0..columns {
        let b_start = start + (col * n) / columns;
        let b_end = start + ((col + 1) * n) / columns;

        let open = opens[b_start];
        let close = closes[b_end - 1];

        let mut high = highs[b_start];
        let mut low = lows[b_start];

        for idx in (b_start + 1)..b_end {
            let h = highs[idx];
            let l = lows[idx];
            if high.is_nan() || h.is_nan() {
                high = f64::NAN;
            } else if h > high {
                high = h;
            }
            if low.is_nan() || l.is_nan() {
                low = f64::NAN;
            } else if l < low {
                low = l;
            }
        }

        out.push(Bucket {
            start: b_start,
            end: b_end,
            open,
            high,
            low,
            close,
        });
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::{BarColumns, TimeLabel};

    fn make_frame(
        times: &[i64],
        opens: &[f64],
        highs: &[f64],
        lows: &[f64],
        closes: &[f64],
    ) -> BarFrame {
        BarFrame::try_new(BarColumns {
            time: times.to_vec(),
            open: opens.to_vec(),
            high: highs.to_vec(),
            low: lows.to_vec(),
            close: closes.to_vec(),
            tick_volume: None,
            spread: None,
            real_volume: None,
            label: TimeLabel::Utc,
        })
        .unwrap()
    }

    fn synthetic_frame(n: usize) -> BarFrame {
        let times: Vec<i64> = (0..n).map(|i| (i * 60) as i64).collect();
        let opens: Vec<f64> = (0..n).map(|i| (i as f64) + 10.0).collect();
        let highs: Vec<f64> = (0..n).map(|i| (i as f64) + 15.0).collect();
        let lows: Vec<f64> = (0..n).map(|i| (i as f64) + 5.0).collect();
        let closes: Vec<f64> = (0..n).map(|i| (i as f64) + 12.0).collect();
        make_frame(&times, &opens, &highs, &lows, &closes)
    }

    #[test]
    fn test_1000_bars_into_300_columns() {
        let frame = synthetic_frame(1000);
        let buckets = reduce(&frame, 0..1000, 300).unwrap();
        assert_eq!(buckets.len(), 300);

        for (col, b) in buckets.iter().enumerate() {
            let expected_start = (col * 1000) / 300;
            let expected_end = ((col + 1) * 1000) / 300;
            assert_eq!(b.start, expected_start);
            assert_eq!(b.end, expected_end);
            assert!(b.end > b.start);
        }
    }

    #[test]
    fn test_same_call_twice_gives_identical_buckets() {
        let frame = synthetic_frame(1000);
        let b1 = reduce(&frame, 100..800, 250).unwrap();
        let b2 = reduce(&frame, 100..800, 250).unwrap();
        assert_eq!(b1.len(), b2.len());
        for (x, y) in b1.iter().zip(b2.iter()) {
            assert_eq!(x.start, y.start);
            assert_eq!(x.end, y.end);
            assert_eq!(x.open.to_bits(), y.open.to_bits());
            assert_eq!(x.high.to_bits(), y.high.to_bits());
            assert_eq!(x.low.to_bits(), y.low.to_bits());
            assert_eq!(x.close.to_bits(), y.close.to_bits());
        }
    }

    #[test]
    fn test_100_bars_into_300_columns() {
        let frame = synthetic_frame(100);
        let buckets = reduce(&frame, 0..100, 300).unwrap();
        assert_eq!(buckets.len(), 100);

        for (i, b) in buckets.iter().enumerate() {
            assert_eq!(b.start, i);
            assert_eq!(b.end, i + 1);
            assert_eq!(b.open.to_bits(), frame.open()[i].to_bits());
            assert_eq!(b.high.to_bits(), frame.high()[i].to_bits());
            assert_eq!(b.low.to_bits(), frame.low()[i].to_bits());
            assert_eq!(b.close.to_bits(), frame.close()[i].to_bits());
        }
    }

    #[test]
    fn test_extremes_exact_and_nan_propagates() {
        let times = vec![1, 2, 3, 4];
        let opens = vec![10.0, 12.0, 11.0, 13.0];
        let highs = vec![15.0, 25.0, 18.0, 20.0];
        let lows = vec![8.0, 9.0, 4.0, 7.0];
        let closes = vec![12.0, 11.0, 13.0, 14.0];
        let frame = make_frame(&times, &opens, &highs, &lows, &closes);

        let buckets = reduce(&frame, 0..4, 1).unwrap();
        assert_eq!(buckets.len(), 1);
        assert_eq!(buckets[0].open.to_bits(), 10.0f64.to_bits());
        assert_eq!(buckets[0].high.to_bits(), 25.0f64.to_bits());
        assert_eq!(buckets[0].low.to_bits(), 4.0f64.to_bits());
        assert_eq!(buckets[0].close.to_bits(), 14.0f64.to_bits());

        // NaN high propagates
        let highs_nan = vec![15.0, f64::NAN, 18.0, 20.0];
        let frame_nan = make_frame(&times, &opens, &highs_nan, &lows, &closes);
        let buckets_nan = reduce(&frame_nan, 0..4, 1).unwrap();
        assert!(buckets_nan[0].high.is_nan());
        assert_eq!(buckets_nan[0].low.to_bits(), 4.0f64.to_bits());
    }

    #[test]
    fn test_empty_range_gives_no_buckets() {
        let frame = synthetic_frame(50);
        let b1 = reduce(&frame, 10..10, 100).unwrap();
        assert!(b1.is_empty());

        let b2 = reduce(&frame, std::ops::Range { start: 20, end: 10 }, 100).unwrap();
        assert!(b2.is_empty());

        let b3 = reduce(&frame, 60..80, 100).unwrap();
        assert!(b3.is_empty());
    }

    #[test]
    fn test_zero_columns_is_error() {
        let frame = synthetic_frame(50);
        assert_eq!(reduce(&frame, 0..50, 0), Err(SeriesError::Empty));
    }

    #[test]
    fn test_reduce_into_pre_sized_allocates_nothing() {
        let frame = synthetic_frame(1000);
        let mut out = Vec::with_capacity(300);
        reduce_into(&frame, 0..1000, 300, &mut out).unwrap();
        assert_eq!(out.len(), 300);
        let initial_cap = out.capacity();
        let initial_ptr = out.as_ptr();

        for _ in 0..10 {
            reduce_into(&frame, 0..1000, 300, &mut out).unwrap();
            assert_eq!(out.len(), 300);
            assert_eq!(out.capacity(), initial_cap);
            assert_eq!(out.as_ptr(), initial_ptr);
        }
    }
}
