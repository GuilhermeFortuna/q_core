//! Growing display series for one symbol and timeframe, with a forming bar held apart.

use crate::frame::{BarColumns, BarFrame, FrameError, TimeLabel, VolumeSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Revision(pub u64);

impl Revision {
    pub fn as_u64(self) -> u64 {
        self.0
    }
}

/// Bars changed since a caller's revision. `TooOld` means "refresh everything".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirtyRange {
    None,
    Bars { start: usize, end: usize },
    TooOld,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SeriesExtents {
    pub first_time: i64,
    pub last_time: i64,
    pub low: f64,
    pub high: f64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SeriesError {
    NotAscending { at: usize },
    BeforeLast { at: usize },
    Frame(FrameError),
    Empty,
}

impl From<FrameError> for SeriesError {
    fn from(err: FrameError) -> Self {
        Self::Frame(err)
    }
}

/// Growing display series for one symbol and timeframe, with a forming bar held
/// apart. Distinct from `RollingBarWindow`: this one refuses an out-of-order
/// batch instead of sorting it.
pub struct LiveBarSeries {
    capacity: usize,
    volumes: VolumeSet,
    label: TimeLabel,
    completed: BarFrame,
    forming: Option<BarFrame>,
    revision: Revision,
    horizon: Revision,
    recent_dirty: Vec<(Revision, usize, usize)>,
}

impl LiveBarSeries {
    pub fn new(capacity: usize, volumes: VolumeSet, label: TimeLabel) -> Result<Self, FrameError> {
        if capacity == 0 {
            return Err(FrameError::ZeroBound);
        }
        let empty = BarFrame::try_new(BarColumns {
            time: Vec::new(),
            open: Vec::new(),
            high: Vec::new(),
            low: Vec::new(),
            close: Vec::new(),
            tick_volume: volumes.tick_volume.then(Vec::new),
            spread: volumes.spread.then(Vec::new),
            real_volume: volumes.real_volume.then(Vec::new),
            label,
        })?;
        Ok(Self {
            capacity,
            volumes,
            label,
            completed: empty,
            forming: None,
            revision: Revision(0),
            horizon: Revision(0),
            recent_dirty: Vec::new(),
        })
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn len(&self) -> usize {
        self.completed.len()
    }

    pub fn is_empty(&self) -> bool {
        self.completed.is_empty()
    }

    pub fn has_forming(&self) -> bool {
        self.forming.is_some()
    }

    pub fn revision(&self) -> Revision {
        self.revision
    }

    pub fn completed(&self) -> &BarFrame {
        &self.completed
    }

    pub fn forming(&self) -> Option<&BarFrame> {
        self.forming.as_ref()
    }

    pub fn last_close(&self) -> Option<f64> {
        if let Some(f) = &self.forming {
            Some(f.close()[0])
        } else {
            self.completed.close().last().copied()
        }
    }

    pub fn extents(&self) -> Option<SeriesExtents> {
        if self.completed.is_empty() {
            if let Some(f) = &self.forming {
                return Some(SeriesExtents {
                    first_time: f.time()[0],
                    last_time: f.time()[0],
                    low: f.low()[0],
                    high: f.high()[0],
                });
            }
            return None;
        }

        let first_time = self.completed.time()[0];
        let last_time = if let Some(f) = &self.forming {
            f.time()[0]
        } else {
            *self.completed.time().last().unwrap()
        };

        let mut low = self.completed.low()[0];
        let mut high = self.completed.high()[0];

        for &l in self.completed.low() {
            if low.is_nan() || l.is_nan() {
                low = f64::NAN;
            } else if l < low {
                low = l;
            }
        }
        for &h in self.completed.high() {
            if high.is_nan() || h.is_nan() {
                high = f64::NAN;
            } else if h > high {
                high = h;
            }
        }

        if let Some(f) = &self.forming {
            let fl = f.low()[0];
            let fh = f.high()[0];
            if low.is_nan() || fl.is_nan() {
                low = f64::NAN;
            } else if fl < low {
                low = fl;
            }
            if high.is_nan() || fh.is_nan() {
                high = f64::NAN;
            } else if fh > high {
                high = fh;
            }
        }

        Some(SeriesExtents {
            first_time,
            last_time,
            low,
            high,
        })
    }

    pub fn load_history(&mut self, mut bars: BarColumns) -> Result<(), SeriesError> {
        self.check_batch_meta(&bars)?;
        validate_column_lens(&bars)?;
        if bars.time.is_empty() {
            return Err(SeriesError::Empty);
        }
        for i in 1..bars.time.len() {
            if bars.time[i] <= bars.time[i - 1] {
                return Err(SeriesError::NotAscending { at: i });
            }
        }

        if bars.time.len() > self.capacity {
            let drop_n = bars.time.len() - self.capacity;
            drain_bars_front(&mut bars, drop_n);
        }

        let new_frame = BarFrame::try_new(bars)?;
        self.completed = new_frame;

        if let Some(f) = &self.forming {
            if let Some(&last_t) = self.completed.time().last() {
                if last_t >= f.time()[0] {
                    self.forming = None;
                }
            }
        }

        self.revision.0 += 1;
        self.horizon = self.revision;
        self.recent_dirty.clear();
        Ok(())
    }

    pub fn append_completed(&mut self, batch: BarColumns) -> Result<(), SeriesError> {
        self.check_batch_meta(&batch)?;
        validate_column_lens(&batch)?;
        if batch.time.is_empty() {
            return Err(SeriesError::Empty);
        }
        for i in 1..batch.time.len() {
            if batch.time[i] <= batch.time[i - 1] {
                return Err(SeriesError::NotAscending { at: i });
            }
        }

        let cur_len = self.completed.len();
        let (dirty_start, dirty_end) = if cur_len == 0 {
            append_frame_bars(&mut self.completed, &batch);
            (0, batch.time.len())
        } else {
            let last_t = *self.completed.time().last().unwrap();
            let first_batch_t = batch.time[0];
            if first_batch_t < last_t {
                return Err(SeriesError::BeforeLast { at: 0 });
            } else if first_batch_t == last_t {
                let replace_idx = cur_len - 1;
                replace_last_bar(&mut self.completed, replace_idx, &batch);
                if batch.time.len() > 1 {
                    append_frame_slice(&mut self.completed, &batch, 1);
                }
                (replace_idx, replace_idx + batch.time.len())
            } else {
                append_frame_bars(&mut self.completed, &batch);
                (cur_len, cur_len + batch.time.len())
            }
        };

        let mut capacity_dropped = false;
        if self.completed.len() > self.capacity {
            let drop_n = self.completed.len() - self.capacity;
            drain_frame_front(&mut self.completed, drop_n);
            capacity_dropped = true;
        }

        if let Some(f) = &self.forming {
            if let Some(&last_t) = self.completed.time().last() {
                if last_t >= f.time()[0] {
                    self.forming = None;
                }
            }
        }

        self.revision.0 += 1;
        if capacity_dropped {
            self.horizon = self.revision;
            self.recent_dirty.clear();
        } else {
            self.record_dirty(self.revision, dirty_start, dirty_end);
        }

        Ok(())
    }

    pub fn set_forming(&mut self, bar: BarColumns) -> Result<(), SeriesError> {
        self.check_batch_meta(&bar)?;
        validate_column_lens(&bar)?;
        if bar.time.is_empty() {
            return Err(SeriesError::Empty);
        }
        if bar.time.len() != 1 {
            return Err(SeriesError::Frame(FrameError::FormingNotSingleBar {
                rows: bar.time.len(),
            }));
        }
        let forming_time = bar.time[0];
        if let Some(&last_t) = self.completed.time().last() {
            if forming_time <= last_t {
                return Err(SeriesError::BeforeLast { at: 0 });
            }
        }

        let forming_idx = self.completed.len();
        if let Some(existing) = &mut self.forming {
            let fbars = existing.bars_mut();
            fbars.time[0] = bar.time[0];
            fbars.open[0] = bar.open[0];
            fbars.high[0] = bar.high[0];
            fbars.low[0] = bar.low[0];
            fbars.close[0] = bar.close[0];
            if let (Some(dst), Some(src)) = (fbars.tick_volume.as_mut(), bar.tick_volume.as_ref()) {
                dst[0] = src[0];
            }
            if let (Some(dst), Some(src)) = (fbars.spread.as_mut(), bar.spread.as_ref()) {
                dst[0] = src[0];
            }
            if let (Some(dst), Some(src)) = (fbars.real_volume.as_mut(), bar.real_volume.as_ref()) {
                dst[0] = src[0];
            }
        } else {
            self.forming = Some(BarFrame::try_new(bar)?);
        }

        self.revision.0 += 1;
        self.record_dirty(self.revision, forming_idx, forming_idx + 1);
        Ok(())
    }

    pub fn clear_forming(&mut self) {
        if self.forming.is_some() {
            let forming_idx = self.completed.len();
            self.forming = None;
            self.revision.0 += 1;
            self.record_dirty(self.revision, forming_idx, forming_idx + 1);
        }
    }

    pub fn dirty_since(&self, since: Revision) -> DirtyRange {
        if since == self.revision {
            return DirtyRange::None;
        }
        if since < self.horizon || since > self.revision {
            return DirtyRange::TooOld;
        }
        let matching: Vec<_> = self
            .recent_dirty
            .iter()
            .filter(|(rev, _, _)| *rev > since && *rev <= self.revision)
            .collect();

        if matching.is_empty() {
            return DirtyRange::TooOld;
        }

        let mut min_start = usize::MAX;
        let mut max_end = 0;
        for &(_, start, end) in matching {
            min_start = min_start.min(start);
            max_end = max_end.max(end);
        }
        DirtyRange::Bars {
            start: min_start,
            end: max_end,
        }
    }

    fn check_batch_meta(&self, batch: &BarColumns) -> Result<(), FrameError> {
        let actual = VolumeSet::from_bars(batch);
        if actual != self.volumes {
            return Err(FrameError::VolumeSetMismatch {
                expected: self.volumes,
                actual,
            });
        }
        if batch.label != self.label {
            return Err(FrameError::LabelMismatch {
                expected: self.label,
                actual: batch.label,
            });
        }
        Ok(())
    }

    fn record_dirty(&mut self, rev: Revision, start: usize, end: usize) {
        if self.recent_dirty.len() >= 256 {
            self.recent_dirty.drain(0..128);
            if let Some(&(oldest_rev, _, _)) = self.recent_dirty.first() {
                if oldest_rev > self.horizon {
                    self.horizon = oldest_rev;
                }
            }
        }
        self.recent_dirty.push((rev, start, end));
    }
}

fn validate_column_lens(bars: &BarColumns) -> Result<(), FrameError> {
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

fn drain_bars_front(bars: &mut BarColumns, drop_n: usize) {
    if drop_n == 0 {
        return;
    }
    bars.time.drain(0..drop_n);
    bars.open.drain(0..drop_n);
    bars.high.drain(0..drop_n);
    bars.low.drain(0..drop_n);
    bars.close.drain(0..drop_n);
    if let Some(v) = bars.tick_volume.as_mut() {
        v.drain(0..drop_n);
    }
    if let Some(v) = bars.spread.as_mut() {
        v.drain(0..drop_n);
    }
    if let Some(v) = bars.real_volume.as_mut() {
        v.drain(0..drop_n);
    }
}

fn drain_frame_front(frame: &mut BarFrame, drop_n: usize) {
    drain_bars_front(frame.bars_mut(), drop_n);
}

fn append_frame_bars(frame: &mut BarFrame, batch: &BarColumns) {
    let bars = frame.bars_mut();
    bars.time.extend_from_slice(&batch.time);
    bars.open.extend_from_slice(&batch.open);
    bars.high.extend_from_slice(&batch.high);
    bars.low.extend_from_slice(&batch.low);
    bars.close.extend_from_slice(&batch.close);
    if let (Some(dst), Some(src)) = (bars.tick_volume.as_mut(), batch.tick_volume.as_ref()) {
        dst.extend_from_slice(src);
    }
    if let (Some(dst), Some(src)) = (bars.spread.as_mut(), batch.spread.as_ref()) {
        dst.extend_from_slice(src);
    }
    if let (Some(dst), Some(src)) = (bars.real_volume.as_mut(), batch.real_volume.as_ref()) {
        dst.extend_from_slice(src);
    }
}

fn append_frame_slice(frame: &mut BarFrame, batch: &BarColumns, start: usize) {
    let bars = frame.bars_mut();
    bars.time.extend_from_slice(&batch.time[start..]);
    bars.open.extend_from_slice(&batch.open[start..]);
    bars.high.extend_from_slice(&batch.high[start..]);
    bars.low.extend_from_slice(&batch.low[start..]);
    bars.close.extend_from_slice(&batch.close[start..]);
    if let (Some(dst), Some(src)) = (bars.tick_volume.as_mut(), batch.tick_volume.as_ref()) {
        dst.extend_from_slice(&src[start..]);
    }
    if let (Some(dst), Some(src)) = (bars.spread.as_mut(), batch.spread.as_ref()) {
        dst.extend_from_slice(&src[start..]);
    }
    if let (Some(dst), Some(src)) = (bars.real_volume.as_mut(), batch.real_volume.as_ref()) {
        dst.extend_from_slice(&src[start..]);
    }
}

fn replace_last_bar(frame: &mut BarFrame, idx: usize, batch: &BarColumns) {
    let bars = frame.bars_mut();
    bars.time[idx] = batch.time[0];
    bars.open[idx] = batch.open[0];
    bars.high[idx] = batch.high[0];
    bars.low[idx] = batch.low[0];
    bars.close[idx] = batch.close[0];
    if let (Some(dst), Some(src)) = (bars.tick_volume.as_mut(), batch.tick_volume.as_ref()) {
        dst[idx] = src[0];
    }
    if let (Some(dst), Some(src)) = (bars.spread.as_mut(), batch.spread.as_ref()) {
        dst[idx] = src[0];
    }
    if let (Some(dst), Some(src)) = (bars.real_volume.as_mut(), batch.real_volume.as_ref()) {
        dst[idx] = src[0];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vols_none() -> VolumeSet {
        VolumeSet {
            tick_volume: false,
            spread: false,
            real_volume: false,
        }
    }

    fn make_bars(
        times: &[i64],
        opens: &[f64],
        highs: &[f64],
        lows: &[f64],
        closes: &[f64],
    ) -> BarColumns {
        BarColumns {
            time: times.to_vec(),
            open: opens.to_vec(),
            high: highs.to_vec(),
            low: lows.to_vec(),
            close: closes.to_vec(),
            tick_volume: None,
            spread: None,
            real_volume: None,
            label: TimeLabel::Utc,
        }
    }

    fn simple_bars(times: &[i64], close_val: f64) -> BarColumns {
        let n = times.len();
        make_bars(
            times,
            &vec![close_val; n],
            &vec![close_val + 1.0; n],
            &vec![close_val - 1.0; n],
            &vec![close_val; n],
        )
    }

    fn assert_f64_eq(actual: &[f64], expected: &[f64]) {
        assert_eq!(actual.len(), expected.len());
        for (a, e) in actual.iter().zip(expected.iter()) {
            assert_eq!(a.to_bits(), e.to_bits());
        }
    }

    #[test]
    fn test_append_extends() {
        let mut series = LiveBarSeries::new(10, vols_none(), TimeLabel::Utc).unwrap();
        series
            .append_completed(simple_bars(&[100, 200], 10.0))
            .unwrap();
        assert_eq!(series.len(), 2);
        assert_eq!(series.completed().time(), &[100, 200]);

        series
            .append_completed(simple_bars(&[300, 400], 20.0))
            .unwrap();
        assert_eq!(series.len(), 4);
        assert_eq!(series.completed().time(), &[100, 200, 300, 400]);
        assert_f64_eq(series.completed().close(), &[10.0, 10.0, 20.0, 20.0]);
    }

    #[test]
    fn test_equal_time_replaces_last() {
        let mut series = LiveBarSeries::new(10, vols_none(), TimeLabel::Utc).unwrap();
        series
            .append_completed(simple_bars(&[100, 200], 10.0))
            .unwrap();
        assert_eq!(series.len(), 2);

        // Append batch starting at 200 with new close 15.0 and next bar at 300 with close 25.0
        series
            .append_completed(simple_bars(&[200, 300], 15.0))
            .unwrap();
        assert_eq!(series.len(), 3);
        assert_eq!(series.completed().time(), &[100, 200, 300]);
        assert_f64_eq(series.completed().close(), &[10.0, 15.0, 15.0]);
    }

    #[test]
    fn test_before_last_refused() {
        let mut series = LiveBarSeries::new(10, vols_none(), TimeLabel::Utc).unwrap();
        series
            .append_completed(simple_bars(&[100, 200], 10.0))
            .unwrap();
        let rev_before = series.revision();

        // Batch starting before last (150 < 200)
        let res = series.append_completed(simple_bars(&[150, 250], 12.0));
        assert_eq!(res, Err(SeriesError::BeforeLast { at: 0 }));
        assert_eq!(series.len(), 2);
        assert_eq!(series.revision(), rev_before);
    }

    #[test]
    fn test_not_ascending_refused() {
        let mut series = LiveBarSeries::new(10, vols_none(), TimeLabel::Utc).unwrap();
        // Internally unordered batch
        let res = series.append_completed(simple_bars(&[100, 100], 10.0));
        assert_eq!(res, Err(SeriesError::NotAscending { at: 1 }));
        assert_eq!(series.len(), 0);

        let res2 = series.append_completed(simple_bars(&[100, 50], 10.0));
        assert_eq!(res2, Err(SeriesError::NotAscending { at: 1 }));
    }

    #[test]
    fn test_capacity_drops_oldest() {
        let mut series = LiveBarSeries::new(3, vols_none(), TimeLabel::Utc).unwrap();
        series
            .append_completed(simple_bars(&[10, 20, 30, 40, 50], 1.0))
            .unwrap();
        assert_eq!(series.len(), 3);
        assert_eq!(series.completed().time(), &[30, 40, 50]);

        // Append another bar
        series.append_completed(simple_bars(&[60], 2.0)).unwrap();
        assert_eq!(series.len(), 3);
        assert_eq!(series.completed().time(), &[40, 50, 60]);
    }

    #[test]
    fn test_forming_replaced_leaves_completed_identical() {
        let mut series = LiveBarSeries::new(10, vols_none(), TimeLabel::Utc).unwrap();
        series
            .append_completed(simple_bars(&[100, 200], 10.0))
            .unwrap();

        let completed_times = series.completed().time().to_vec();
        let completed_closes = series.completed().close().to_vec();
        let time_ptr = series.completed().time().as_ptr();

        for i in 1..=1000 {
            series.set_forming(simple_bars(&[300], i as f64)).unwrap();
            assert_eq!(
                series.forming().unwrap().close()[0].to_bits(),
                (i as f64).to_bits()
            );
            assert!(series.has_forming());
        }

        assert_eq!(series.completed().time(), &completed_times[..]);
        assert_f64_eq(series.completed().close(), &completed_closes[..]);
        assert_eq!(series.completed().time().as_ptr(), time_ptr);
    }

    #[test]
    fn test_completed_at_forming_time_clears_forming() {
        let mut series = LiveBarSeries::new(10, vols_none(), TimeLabel::Utc).unwrap();
        series.append_completed(simple_bars(&[100], 10.0)).unwrap();
        series.set_forming(simple_bars(&[200], 12.0)).unwrap();
        assert!(series.has_forming());

        // Completed bar at forming time 200
        series.append_completed(simple_bars(&[200], 13.0)).unwrap();
        assert!(!series.has_forming());
        assert_eq!(series.completed().time(), &[100, 200]);
        assert_f64_eq(series.completed().close(), &[10.0, 13.0]);
    }

    #[test]
    fn test_failed_load_history_leaves_previous_intact() {
        let mut series = LiveBarSeries::new(10, vols_none(), TimeLabel::Utc).unwrap();
        series
            .append_completed(simple_bars(&[100, 200], 10.0))
            .unwrap();
        let rev_before = series.revision();

        // Attempt to load invalid history
        let res = series.load_history(simple_bars(&[100, 50], 5.0));
        assert_eq!(res, Err(SeriesError::NotAscending { at: 1 }));

        assert_eq!(series.len(), 2);
        assert_eq!(series.completed().time(), &[100, 200]);
        assert_eq!(series.revision(), rev_before);
    }

    #[test]
    fn test_revision_advances_only_on_accepted_mutation() {
        let mut series = LiveBarSeries::new(10, vols_none(), TimeLabel::Utc).unwrap();
        let r0 = series.revision();

        // Refused append (empty or not ascending)
        assert!(series.append_completed(simple_bars(&[], 1.0)).is_err());
        assert_eq!(series.revision(), r0);

        assert!(series
            .append_completed(simple_bars(&[100, 100], 1.0))
            .is_err());
        assert_eq!(series.revision(), r0);

        // Accepted append
        series.append_completed(simple_bars(&[100], 1.0)).unwrap();
        let r1 = series.revision();
        assert!(r1 > r0);

        // Refused append before last
        assert!(series.append_completed(simple_bars(&[50], 1.0)).is_err());
        assert_eq!(series.revision(), r1);

        // Refused forming (stale)
        assert!(series.set_forming(simple_bars(&[100], 2.0)).is_err());
        assert_eq!(series.revision(), r1);

        // Accepted forming
        series.set_forming(simple_bars(&[200], 2.0)).unwrap();
        let r2 = series.revision();
        assert!(r2 > r1);

        // Clear forming
        series.clear_forming();
        let r3 = series.revision();
        assert!(r3 > r2);

        // Clear forming again (no-op)
        series.clear_forming();
        assert_eq!(series.revision(), r3);

        // Refused load history
        assert!(series.load_history(simple_bars(&[10, 5], 1.0)).is_err());
        assert_eq!(series.revision(), r3);

        // Accepted load history
        series.load_history(simple_bars(&[10, 20], 1.0)).unwrap();
        let r4 = series.revision();
        assert!(r4 > r3);
    }

    #[test]
    fn test_dirty_since_append_replace_forming() {
        let mut series = LiveBarSeries::new(20, vols_none(), TimeLabel::Utc).unwrap();
        series
            .load_history(simple_bars(&[100, 200, 300, 400, 500], 10.0))
            .unwrap();
        let r_hist = series.revision();
        assert_eq!(series.dirty_since(r_hist), DirtyRange::None);

        // Append 2 bars
        series
            .append_completed(simple_bars(&[600, 700], 20.0))
            .unwrap();
        let r_app = series.revision();
        assert_eq!(
            series.dirty_since(r_hist),
            DirtyRange::Bars { start: 5, end: 7 }
        );
        assert_eq!(series.dirty_since(r_app), DirtyRange::None);

        // Replace last bar at 700
        series.append_completed(simple_bars(&[700], 25.0)).unwrap();
        let r_rep = series.revision();
        assert_eq!(
            series.dirty_since(r_app),
            DirtyRange::Bars { start: 6, end: 7 }
        );
        assert_eq!(
            series.dirty_since(r_hist),
            DirtyRange::Bars { start: 5, end: 7 }
        );
        assert_eq!(series.dirty_since(r_rep), DirtyRange::None);

        // Set forming bar at 800
        series.set_forming(simple_bars(&[800], 30.0)).unwrap();
        let r_form = series.revision();
        assert_eq!(
            series.dirty_since(r_rep),
            DirtyRange::Bars { start: 7, end: 8 }
        );
        assert_eq!(
            series.dirty_since(r_app),
            DirtyRange::Bars { start: 6, end: 8 }
        );
        assert_eq!(
            series.dirty_since(r_hist),
            DirtyRange::Bars { start: 5, end: 8 }
        );
        assert_eq!(series.dirty_since(r_form), DirtyRange::None);
    }

    #[test]
    fn test_dirty_since_too_old_after_load_history_and_capacity_drop() {
        let mut series = LiveBarSeries::new(10, vols_none(), TimeLabel::Utc).unwrap();
        series
            .append_completed(simple_bars(&[100, 200], 10.0))
            .unwrap();
        let r_prev = series.revision();

        // load_history makes any prior revision TooOld
        series
            .load_history(simple_bars(&[300, 400, 500], 15.0))
            .unwrap();
        assert_eq!(series.dirty_since(r_prev), DirtyRange::TooOld);
        assert_eq!(series.dirty_since(series.revision()), DirtyRange::None);

        // capacity drop makes prior revisions TooOld
        let mut bounded = LiveBarSeries::new(3, vols_none(), TimeLabel::Utc).unwrap();
        bounded
            .append_completed(simple_bars(&[10, 20, 30], 1.0))
            .unwrap();
        let r_full = bounded.revision();

        bounded.append_completed(simple_bars(&[40], 2.0)).unwrap();
        assert_eq!(bounded.dirty_since(r_full), DirtyRange::TooOld);
        assert_eq!(bounded.dirty_since(bounded.revision()), DirtyRange::None);
    }
}
