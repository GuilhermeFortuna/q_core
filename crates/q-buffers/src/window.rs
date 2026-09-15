//! Bounded completed-bar window with at most one forming bar.

use crate::frame::{BarColumns, BarFrame, FrameError, TimeLabel, VolumeSet};

/// Bounded completed-bar window equal to `StrategyEvaluator`'s pandas window for completed bars.
#[derive(Clone, Debug)]
pub struct RollingBarWindow {
    bound: usize,
    volumes: VolumeSet,
    label: TimeLabel,
    completed: BarFrame,
    forming: Option<BarFrame>,
}

impl RollingBarWindow {
    pub fn new(bound: usize, volumes: VolumeSet, label: TimeLabel) -> Result<Self, FrameError> {
        if bound == 0 {
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
            bound,
            volumes,
            label,
            completed: empty,
            forming: None,
        })
    }

    pub fn bound(&self) -> usize {
        self.bound
    }

    pub fn len(&self) -> usize {
        self.completed.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn ingest_completed(&mut self, batch: BarColumns) -> Result<(), FrameError> {
        self.check_batch_meta(&batch)?;
        if batch.time.is_empty() {
            return Ok(());
        }

        let can_append = self.completed.is_empty()
            || (is_strictly_increasing(&batch.time)
                && batch.time[0] > *self.completed.time().last().unwrap());

        if can_append {
            append_bars(&mut self.completed, &batch);
        } else {
            self.completed = merge_keep_last(self.completed.bars(), &batch, self.bound)?;
        }

        if self.completed.len() > self.bound {
            let drop_n = self.completed.len() - self.bound;
            drain_front(&mut self.completed, drop_n);
        }

        if let Some(forming) = &self.forming {
            let newest = *self.completed.time().last().unwrap();
            let forming_time = forming.time()[0];
            if newest >= forming_time {
                self.forming = None;
            }
        }
        Ok(())
    }

    pub fn set_forming(&mut self, bar: BarColumns) -> Result<(), FrameError> {
        self.check_batch_meta(&bar)?;
        if bar.time.len() != 1 {
            return Err(FrameError::FormingNotSingleBar {
                rows: bar.time.len(),
            });
        }
        if let Some(&newest) = self.completed.time().last() {
            if bar.time[0] <= newest {
                return Err(FrameError::StaleForming {
                    forming_time: bar.time[0],
                    newest_completed: newest,
                });
            }
        }
        self.forming = Some(BarFrame::try_new(bar)?);
        Ok(())
    }

    pub fn clear_forming(&mut self) {
        self.forming = None;
    }

    pub fn completed(&self) -> &BarFrame {
        &self.completed
    }

    pub fn forming(&self) -> Option<&BarFrame> {
        self.forming.as_ref()
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
}

fn is_strictly_increasing(times: &[i64]) -> bool {
    times.windows(2).all(|w| w[1] > w[0])
}

fn append_bars(frame: &mut BarFrame, batch: &BarColumns) {
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

fn drain_front(frame: &mut BarFrame, drop_n: usize) {
    if drop_n == 0 {
        return;
    }
    let bars = frame.bars_mut();
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

fn merge_keep_last(
    existing: &BarColumns,
    batch: &BarColumns,
    bound: usize,
) -> Result<BarFrame, FrameError> {
    let total = existing.time.len() + batch.time.len();
    let mut times = Vec::with_capacity(total);
    times.extend_from_slice(&existing.time);
    times.extend_from_slice(&batch.time);

    let mut order: Vec<usize> = (0..total).collect();
    order.sort_by_key(|&i| times[i]);

    // Keep last occurrence of each time (later in concatenation wins).
    let mut keep = Vec::with_capacity(total);
    let mut i = 0;
    while i < order.len() {
        let mut j = i + 1;
        while j < order.len() && times[order[j]] == times[order[i]] {
            j += 1;
        }
        keep.push(order[j - 1]);
        i = j;
    }
    if keep.len() > bound {
        keep = keep[keep.len() - bound..].to_vec();
    }

    let pick = |src_exist: &[f64], src_batch: &[f64], idx: usize| -> f64 {
        if idx < existing.time.len() {
            src_exist[idx]
        } else {
            src_batch[idx - existing.time.len()]
        }
    };
    let pick_i64 = |src_exist: Option<&[i64]>, src_batch: Option<&[i64]>, idx: usize| -> i64 {
        if idx < existing.time.len() {
            src_exist.unwrap()[idx]
        } else {
            src_batch.unwrap()[idx - existing.time.len()]
        }
    };

    let mut out = BarColumns {
        time: Vec::with_capacity(keep.len()),
        open: Vec::with_capacity(keep.len()),
        high: Vec::with_capacity(keep.len()),
        low: Vec::with_capacity(keep.len()),
        close: Vec::with_capacity(keep.len()),
        tick_volume: existing
            .tick_volume
            .as_ref()
            .map(|_| Vec::with_capacity(keep.len())),
        spread: existing
            .spread
            .as_ref()
            .map(|_| Vec::with_capacity(keep.len())),
        real_volume: existing
            .real_volume
            .as_ref()
            .map(|_| Vec::with_capacity(keep.len())),
        label: existing.label,
    };

    for &idx in &keep {
        out.time.push(times[idx]);
        out.open.push(pick(&existing.open, &batch.open, idx));
        out.high.push(pick(&existing.high, &batch.high, idx));
        out.low.push(pick(&existing.low, &batch.low, idx));
        out.close.push(pick(&existing.close, &batch.close, idx));
        if let Some(ref mut v) = out.tick_volume {
            v.push(pick_i64(
                existing.tick_volume.as_deref(),
                batch.tick_volume.as_deref(),
                idx,
            ));
        }
        if let Some(ref mut v) = out.spread {
            v.push(pick_i64(
                existing.spread.as_deref(),
                batch.spread.as_deref(),
                idx,
            ));
        }
        if let Some(ref mut v) = out.real_volume {
            v.push(pick_i64(
                existing.real_volume.as_deref(),
                batch.real_volume.as_deref(),
                idx,
            ));
        }
    }

    BarFrame::try_new(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const H: i64 = 3_600_000_000;

    fn vols_none() -> VolumeSet {
        VolumeSet {
            tick_volume: false,
            spread: false,
            real_volume: false,
        }
    }

    fn bars(times: &[i64], closes: &[f64]) -> BarColumns {
        let n = times.len();
        BarColumns {
            time: times.to_vec(),
            open: vec![1.0; n],
            high: vec![2.0; n],
            low: vec![0.5; n],
            close: closes.to_vec(),
            tick_volume: None,
            spread: None,
            real_volume: None,
            label: TimeLabel::Utc,
        }
    }

    fn ohlc(times: &[i64]) -> BarColumns {
        bars(times, &vec![1.5; times.len()])
    }

    fn window_times(w: &RollingBarWindow) -> Vec<i64> {
        w.completed().time().to_vec()
    }

    #[test]
    fn new_rejects_zero_bound() {
        assert_eq!(
            RollingBarWindow::new(0, vols_none(), TimeLabel::Utc).unwrap_err(),
            FrameError::ZeroBound
        );
    }

    #[test]
    fn rejects_volume_set_mismatch() {
        let mut w = RollingBarWindow::new(20, vols_none(), TimeLabel::Utc).unwrap();
        let mut batch = ohlc(&[H]);
        batch.spread = Some(vec![1]);
        assert!(matches!(
            w.ingest_completed(batch).unwrap_err(),
            FrameError::VolumeSetMismatch { .. }
        ));
    }

    /// Scenario 1: seed 30 → keep bars 10..29 (bound 20).
    #[test]
    fn scenario_seed() {
        let mut w = RollingBarWindow::new(20, vols_none(), TimeLabel::Utc).unwrap();
        let times: Vec<i64> = (0..30).map(|i| i * H).collect();
        w.ingest_completed(ohlc(&times)).unwrap();
        let expected: Vec<i64> = (10..30).map(|i| i * H).collect();
        assert_eq!(window_times(&w), expected);
    }

    /// Scenario 2: after seed 30, ingest bar 30 → bars 11..30.
    #[test]
    fn scenario_append() {
        let mut w = RollingBarWindow::new(20, vols_none(), TimeLabel::Utc).unwrap();
        let seed: Vec<i64> = (0..30).map(|i| i * H).collect();
        w.ingest_completed(ohlc(&seed)).unwrap();
        w.ingest_completed(ohlc(&[30 * H])).unwrap();
        let expected: Vec<i64> = (11..31).map(|i| i * H).collect();
        assert_eq!(window_times(&w), expected);
    }

    /// Scenario 3: re-deliver bar 30 with close + 1.0.
    #[test]
    fn scenario_redeliver() {
        let mut w = RollingBarWindow::new(20, vols_none(), TimeLabel::Utc).unwrap();
        let seed: Vec<i64> = (0..30).map(|i| i * H).collect();
        w.ingest_completed(ohlc(&seed)).unwrap();
        w.ingest_completed(ohlc(&[30 * H])).unwrap();
        w.ingest_completed(bars(&[30 * H], &[2.5])).unwrap();
        assert_eq!(w.len(), 20);
        assert_eq!(w.completed().close()[19].to_bits(), 2.5f64.to_bits());
        let expected: Vec<i64> = (11..31).map(|i| i * H).collect();
        assert_eq!(window_times(&w), expected);
    }

    /// Scenario 4: overlapping batch 28..32 → bars 13..32.
    #[test]
    fn scenario_overlap() {
        let mut w = RollingBarWindow::new(20, vols_none(), TimeLabel::Utc).unwrap();
        let seed: Vec<i64> = (0..30).map(|i| i * H).collect();
        w.ingest_completed(ohlc(&seed)).unwrap();
        w.ingest_completed(ohlc(&[30 * H])).unwrap();
        let batch: Vec<i64> = (28..33).map(|i| i * H).collect();
        w.ingest_completed(ohlc(&batch)).unwrap();
        let expected: Vec<i64> = (13..33).map(|i| i * H).collect();
        assert_eq!(window_times(&w), expected);
    }

    /// Scenario 5: late bar older than a full window → unchanged.
    #[test]
    fn scenario_late_full() {
        let mut w = RollingBarWindow::new(20, vols_none(), TimeLabel::Utc).unwrap();
        let seed: Vec<i64> = (0..30).map(|i| i * H).collect();
        w.ingest_completed(ohlc(&seed)).unwrap();
        w.ingest_completed(ohlc(&[30 * H])).unwrap();
        let before = window_times(&w);
        w.ingest_completed(ohlc(&[5 * H])).unwrap();
        assert_eq!(window_times(&w), before);
    }

    /// Scenario 6: bound 50, seed with gap, then fill → 20 bars in order.
    #[test]
    fn scenario_gap_fill() {
        let mut w = RollingBarWindow::new(50, vols_none(), TimeLabel::Utc).unwrap();
        let mut times: Vec<i64> = (0..10).map(|i| i * H).collect();
        times.extend((12..20).map(|i| i * H));
        w.ingest_completed(ohlc(&times)).unwrap();
        let fill: Vec<i64> = (10..12).map(|i| i * H).collect();
        w.ingest_completed(ohlc(&fill)).unwrap();
        let expected: Vec<i64> = (0..20).map(|i| i * H).collect();
        assert_eq!(window_times(&w), expected);
    }

    /// Scenario 7: unsorted batch with in-batch duplicate; later position wins.
    #[test]
    fn scenario_unsorted_duplicates() {
        let mut w = RollingBarWindow::new(20, vols_none(), TimeLabel::Utc).unwrap();
        let seed: Vec<i64> = (0..30).map(|i| i * H).collect();
        w.ingest_completed(ohlc(&seed)).unwrap();
        // order [33, 31, 33', 32] where 33' has different close
        let batch = bars(&[33 * H, 31 * H, 33 * H, 32 * H], &[1.5, 1.5, 9.0, 1.5]);
        w.ingest_completed(batch).unwrap();
        let times = window_times(&w);
        assert!(times.contains(&(31 * H)));
        assert!(times.contains(&(32 * H)));
        assert!(times.contains(&(33 * H)));
        let idx = times.iter().position(|&t| t == 33 * H).unwrap();
        assert_eq!(w.completed().close()[idx].to_bits(), 9.0f64.to_bits());
    }

    /// Scenario 8: empty batch leaves window unchanged.
    #[test]
    fn scenario_empty() {
        let mut w = RollingBarWindow::new(20, vols_none(), TimeLabel::Utc).unwrap();
        let seed: Vec<i64> = (0..30).map(|i| i * H).collect();
        w.ingest_completed(ohlc(&seed)).unwrap();
        let before = window_times(&w);
        w.ingest_completed(ohlc(&[])).unwrap();
        assert_eq!(window_times(&w), before);
    }

    /// Scenario 9: bound 1 over scenario 2 inputs → bar 30 only.
    #[test]
    fn scenario_bound_one() {
        let mut w = RollingBarWindow::new(1, vols_none(), TimeLabel::Utc).unwrap();
        let seed: Vec<i64> = (0..30).map(|i| i * H).collect();
        w.ingest_completed(ohlc(&seed)).unwrap();
        w.ingest_completed(ohlc(&[30 * H])).unwrap();
        assert_eq!(window_times(&w), vec![30 * H]);
    }

    #[test]
    fn forming_bar_rules() {
        let mut w = RollingBarWindow::new(2, vols_none(), TimeLabel::Utc).unwrap();
        w.ingest_completed(ohlc(&[9 * H, 10 * H])).unwrap();

        w.set_forming(ohlc(&[11 * H])).unwrap();
        assert_eq!(w.forming().unwrap().time(), &[11 * H]);
        assert_eq!(w.len(), 2);

        w.set_forming(bars(&[11 * H], &[3.0])).unwrap();
        assert_eq!(w.forming().unwrap().close()[0].to_bits(), 3.0f64.to_bits());

        assert_eq!(
            w.set_forming(ohlc(&[10 * H])).unwrap_err(),
            FrameError::StaleForming {
                forming_time: 10 * H,
                newest_completed: 10 * H,
            }
        );

        w.ingest_completed(ohlc(&[11 * H])).unwrap();
        assert!(w.forming().is_none());
        assert_eq!(*w.completed().time().last().unwrap(), 11 * H);

        w.set_forming(ohlc(&[12 * H])).unwrap();
        w.ingest_completed(ohlc(&[13 * H])).unwrap();
        assert!(w.forming().is_none());

        w.set_forming(ohlc(&[14 * H])).unwrap();
        assert_eq!(w.len(), 2);

        assert_eq!(
            w.set_forming(ohlc(&[15 * H, 16 * H])).unwrap_err(),
            FrameError::FormingNotSingleBar { rows: 2 }
        );
    }
}
