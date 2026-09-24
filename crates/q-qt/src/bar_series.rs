#![allow(clippy::float_cmp)]
//! CXX-Qt bridge and Rust implementation for LiveBarSeries projected to Qt/QML.

use std::pin::Pin;

use cxx_qt::CxxQtType;
use cxx_qt_lib::QString;
use q_buffers::frame::{BarColumns, TimeLabel, VolumeSet};
use q_buffers::geometry::{
    pack as pack_geometry, pack_bucket, BarVertex as GeomBarVertex, Surface, Viewport,
};
use q_buffers::lod::{reduce_into, Bucket};
use q_buffers::series::{LiveBarSeries, SeriesError};

#[cxx_qt::bridge]
pub mod ffi {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
    }

    struct BarVertex {
        x: f32,
        y: f32,
        direction: f32,
        forming: f32,
    }

    extern "RustQt" {
        #[qobject]
        #[qproperty(QString, symbol)]
        #[qproperty(QString, timeframe)]
        #[qproperty(i64, bar_count)]
        #[qproperty(i64, revision)]
        #[qproperty(f64, last_price)]
        #[qproperty(bool, has_forming)]
        #[qproperty(i64, first_time)]
        #[qproperty(i64, last_time)]
        #[qproperty(f64, low)]
        #[qproperty(f64, high)]
        type BarSeries = super::BarSeriesRust;

        #[qinvokable]
        fn set_viewport(
            self: Pin<&mut BarSeries>,
            first_bar: i64,
            last_bar: i64,
            low: f64,
            high: f64,
        );

        #[qinvokable]
        fn set_surface(self: Pin<&mut BarSeries>, width_px: f32, height_px: f32);

        #[qinvokable]
        fn load_history_sample(self: Pin<&mut BarSeries>, count: i64) -> bool;

        #[qinvokable]
        fn load_history_parquet(self: Pin<&mut BarSeries>, path: &QString) -> bool;

        #[qinvokable]
        fn ingest_completed_bar(
            self: Pin<&mut BarSeries>,
            time: i64,
            open: f64,
            high: f64,
            low: f64,
            close: f64,
            volume: f64,
        ) -> bool;

        #[qinvokable]
        fn ingest_forming_bar(
            self: Pin<&mut BarSeries>,
            time: i64,
            open: f64,
            high: f64,
            low: f64,
            close: f64,
            volume: f64,
        ) -> bool;

        #[qinvokable]
        fn clear_forming_bar(self: Pin<&mut BarSeries>);

        fn vertex_ptr(self: &BarSeries) -> *const BarVertex;

        fn vertex_len(self: &BarSeries) -> usize;

        fn geometry_revision(self: &BarSeries) -> i64;

        fn rebuild_geometry(self: Pin<&mut BarSeries>);

        fn rebuild_split_geometry(self: Pin<&mut BarSeries>);

        fn completed_vertex_ptr(self: &BarSeries) -> *const BarVertex;

        fn completed_vertex_len(self: &BarSeries) -> usize;

        fn completed_geometry_revision(self: &BarSeries) -> i64;

        fn forming_vertex_ptr(self: &BarSeries) -> *const BarVertex;

        fn forming_vertex_len(self: &BarSeries) -> usize;

        fn forming_geometry_revision(self: &BarSeries) -> i64;
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct CompletedCacheKey {
    data_gen: u64,
    first_bar: usize,
    last_bar: usize,
    low: f64,
    high: f64,
    width_px: f32,
    height_px: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct FormingCacheKey {
    data_gen: u64,
    completed_count: usize,
    has_forming: bool,
    visible: bool,
    open: f64,
    high: f64,
    low: f64,
    close: f64,
    first_bar: usize,
    last_bar: usize,
    low_price: f64,
    high_price: f64,
    width_px: f32,
    height_px: f32,
}

fn copy_geom_to_ffi(src: &[GeomBarVertex], dst: &mut Vec<ffi::BarVertex>) {
    dst.clear();
    dst.reserve(src.len());
    for v in src {
        dst.push(ffi::BarVertex {
            x: v.x,
            y: v.y,
            direction: v.direction,
            forming: v.forming,
        });
    }
}

fn forming_output_changed(previous: &[ffi::BarVertex], next: &[GeomBarVertex]) -> bool {
    if previous.len() != next.len() {
        return true;
    }
    previous.iter().zip(next.iter()).any(|(prev, next)| {
        prev.x.to_bits() != next.x.to_bits()
            || prev.y.to_bits() != next.y.to_bits()
            || prev.direction.to_bits() != next.direction.to_bits()
            || prev.forming.to_bits() != next.forming.to_bits()
    })
}

pub struct BarSeriesRust {
    pub symbol: QString,
    pub timeframe: QString,
    pub bar_count: i64,
    pub revision: i64,
    pub last_price: f64,
    pub has_forming: bool,
    pub first_time: i64,
    pub last_time: i64,
    pub low: f64,
    pub high: f64,

    series: LiveBarSeries,
    view_first_bar: i64,
    view_last_bar: i64,
    view_low: f64,
    view_high: f64,
    surface_width: f32,
    surface_height: f32,
    geom_rev: i64,
    buckets: Vec<Bucket>,
    geom_vertices: Vec<GeomBarVertex>,
    vertices: Vec<ffi::BarVertex>,

    completed_data_gen: u64,
    completed_geom_rev: i64,
    forming_geom_rev: i64,
    completed_buckets: Vec<Bucket>,
    completed_geom_vertices: Vec<GeomBarVertex>,
    completed_vertices: Vec<ffi::BarVertex>,
    forming_geom_vertices: Vec<GeomBarVertex>,
    forming_vertices: Vec<ffi::BarVertex>,
    completed_cache: Option<CompletedCacheKey>,
    forming_cache: Option<FormingCacheKey>,
}

impl BarSeriesRust {
    pub fn new(symbol: &str, timeframe: &str, capacity: usize) -> Self {
        let vols = VolumeSet {
            tick_volume: false,
            spread: false,
            real_volume: false,
        };
        let series = LiveBarSeries::new(capacity, vols, TimeLabel::Utc).expect("valid capacity");
        Self {
            symbol: QString::from(symbol),
            timeframe: QString::from(timeframe),
            bar_count: 0,
            revision: 0,
            last_price: 0.0,
            has_forming: false,
            first_time: 0,
            last_time: 0,
            low: 0.0,
            high: 0.0,
            series,
            view_first_bar: 0,
            view_last_bar: 0,
            view_low: 0.0,
            view_high: 0.0,
            surface_width: 0.0,
            surface_height: 0.0,
            geom_rev: 0,
            buckets: Vec::new(),
            geom_vertices: Vec::new(),
            vertices: Vec::new(),
            completed_data_gen: 0,
            completed_geom_rev: 0,
            forming_geom_rev: 0,
            completed_buckets: Vec::new(),
            completed_geom_vertices: Vec::new(),
            completed_vertices: Vec::new(),
            forming_geom_vertices: Vec::new(),
            forming_vertices: Vec::new(),
            completed_cache: None,
            forming_cache: None,
        }
    }

    pub fn load_history(&mut self, bars: BarColumns) -> Result<(), SeriesError> {
        let input_vols = VolumeSet::from_bars(&bars);
        if input_vols != self.series.completed().volume_set() {
            let mut new_series = LiveBarSeries::new(self.series.capacity(), input_vols, bars.label)
                .map_err(SeriesError::Frame)?;
            new_series.load_history(bars)?;
            self.series = new_series;
        } else {
            self.series.load_history(bars)?;
        }
        self.completed_data_gen += 1;
        self.sync_properties();
        Ok(())
    }

    pub fn ingest_completed(&mut self, batch: BarColumns) -> Result<(), SeriesError> {
        self.series.append_completed(batch)?;
        self.completed_data_gen += 1;
        self.sync_properties();
        Ok(())
    }

    pub fn ingest_forming(&mut self, bar: BarColumns) -> Result<(), SeriesError> {
        self.series.set_forming(bar)?;
        self.sync_properties();
        Ok(())
    }

    pub fn clear_forming(&mut self) {
        self.series.clear_forming();
        self.sync_properties();
    }

    pub fn set_viewport(&mut self, first_bar: i64, last_bar: i64, low: f64, high: f64) {
        self.view_first_bar = first_bar;
        self.view_last_bar = last_bar;
        self.view_low = low;
        self.view_high = high;
    }

    pub fn set_surface(&mut self, width_px: f32, height_px: f32) {
        self.surface_width = width_px;
        self.surface_height = height_px;
    }

    pub fn rebuild_geometry(&mut self) {
        if self.surface_width <= 0.0 || self.surface_height <= 0.0 {
            self.vertices.clear();
            return;
        }

        let first_bar = self.view_first_bar.max(0) as usize;
        let last_bar = self.view_last_bar.max(0) as usize;
        if first_bar >= last_bar {
            self.vertices.clear();
            return;
        }

        let columns = (self.surface_width.round() as usize).max(1);
        let _ = reduce_into(
            self.series.completed(),
            first_bar..last_bar,
            columns,
            &mut self.buckets,
        );

        if self.series.has_forming() {
            let forming_idx = self.series.completed().len();
            if first_bar <= forming_idx && forming_idx < last_bar {
                if let Some(forming_frame) = self.series.forming() {
                    self.buckets.push(Bucket {
                        start: forming_idx,
                        end: forming_idx + 1,
                        open: forming_frame.open()[0],
                        high: forming_frame.high()[0],
                        low: forming_frame.low()[0],
                        close: forming_frame.close()[0],
                        forming: true,
                    });
                }
            }
        }

        let view = Viewport {
            first_bar,
            last_bar,
            low: self.view_low,
            high: self.view_high,
        };
        let surface = Surface {
            width_px: self.surface_width,
            height_px: self.surface_height,
        };

        pack_geometry(&self.buckets, view, surface, &mut self.geom_vertices);

        self.vertices.clear();
        self.vertices.reserve(self.geom_vertices.len());
        for v in &self.geom_vertices {
            self.vertices.push(ffi::BarVertex {
                x: v.x,
                y: v.y,
                direction: v.direction,
                forming: v.forming,
            });
        }

        self.geom_rev = self.series.revision().as_u64() as i64;
    }

    pub fn vertex_ptr(&self) -> *const ffi::BarVertex {
        if self.vertices.is_empty() {
            std::ptr::null()
        } else {
            self.vertices.as_ptr()
        }
    }

    pub fn vertex_len(&self) -> usize {
        self.vertices.len()
    }

    pub fn geometry_revision(&self) -> i64 {
        self.geom_rev
    }

    pub fn rebuild_split_geometry(&mut self) {
        let first_bar = self.view_first_bar.max(0) as usize;
        let last_bar = self.view_last_bar.max(0) as usize;
        let surface_invalid = self.surface_width <= 0.0 || self.surface_height <= 0.0;
        let view_invalid = first_bar >= last_bar;

        if surface_invalid || view_invalid {
            if !self.completed_vertices.is_empty() {
                self.completed_geom_vertices.clear();
                self.completed_vertices.clear();
                self.completed_geom_rev += 1;
            }
            if !self.forming_vertices.is_empty() {
                self.forming_geom_vertices.clear();
                self.forming_vertices.clear();
                self.forming_geom_rev += 1;
            }
            self.completed_cache = None;
            self.forming_cache = None;
            return;
        }

        let view = Viewport {
            first_bar,
            last_bar,
            low: self.view_low,
            high: self.view_high,
        };
        let surface = Surface {
            width_px: self.surface_width,
            height_px: self.surface_height,
        };

        let completed_key = CompletedCacheKey {
            data_gen: self.completed_data_gen,
            first_bar,
            last_bar,
            low: self.view_low,
            high: self.view_high,
            width_px: self.surface_width,
            height_px: self.surface_height,
        };

        if self.completed_cache != Some(completed_key) {
            let columns = (self.surface_width.round() as usize).max(1);
            let _ = reduce_into(
                self.series.completed(),
                first_bar..last_bar,
                columns,
                &mut self.completed_buckets,
            );

            self.completed_geom_vertices.clear();
            let bucket_count = self.completed_buckets.len();
            for (i, bucket) in self.completed_buckets.iter().enumerate() {
                pack_bucket(
                    bucket,
                    i,
                    bucket_count,
                    view,
                    surface,
                    &mut self.completed_geom_vertices,
                );
            }
            copy_geom_to_ffi(&self.completed_geom_vertices, &mut self.completed_vertices);
            self.completed_geom_rev += 1;
            self.completed_cache = Some(completed_key);
        }

        let forming_idx = self.series.completed().len();
        let visible =
            self.series.has_forming() && first_bar <= forming_idx && forming_idx < last_bar;

        let (open, high, low, close) = if visible {
            if let Some(forming_frame) = self.series.forming() {
                (
                    forming_frame.open()[0],
                    forming_frame.high()[0],
                    forming_frame.low()[0],
                    forming_frame.close()[0],
                )
            } else {
                (0.0, 0.0, 0.0, 0.0)
            }
        } else {
            (0.0, 0.0, 0.0, 0.0)
        };

        let forming_key = FormingCacheKey {
            data_gen: self.completed_data_gen,
            completed_count: forming_idx,
            has_forming: self.series.has_forming(),
            visible,
            open,
            high,
            low,
            close,
            first_bar,
            last_bar,
            low_price: self.view_low,
            high_price: self.view_high,
            width_px: self.surface_width,
            height_px: self.surface_height,
        };

        if self.forming_cache != Some(forming_key) {
            self.forming_geom_vertices.clear();
            if visible {
                let bucket = Bucket {
                    start: forming_idx,
                    end: forming_idx + 1,
                    open,
                    high,
                    low,
                    close,
                    forming: true,
                };
                pack_bucket(
                    &bucket,
                    0,
                    1,
                    view,
                    surface,
                    &mut self.forming_geom_vertices,
                );
            }
            if forming_output_changed(&self.forming_vertices, &self.forming_geom_vertices) {
                self.forming_geom_rev += 1;
            }
            copy_geom_to_ffi(&self.forming_geom_vertices, &mut self.forming_vertices);
            self.forming_cache = Some(forming_key);
        }
    }

    pub fn completed_vertex_ptr(&self) -> *const ffi::BarVertex {
        if self.completed_vertices.is_empty() {
            std::ptr::null()
        } else {
            self.completed_vertices.as_ptr()
        }
    }

    pub fn completed_vertex_len(&self) -> usize {
        self.completed_vertices.len()
    }

    pub fn completed_geometry_revision(&self) -> i64 {
        self.completed_geom_rev
    }

    pub fn forming_vertex_ptr(&self) -> *const ffi::BarVertex {
        if self.forming_vertices.is_empty() {
            std::ptr::null()
        } else {
            self.forming_vertices.as_ptr()
        }
    }

    pub fn forming_vertex_len(&self) -> usize {
        self.forming_vertices.len()
    }

    pub fn forming_geometry_revision(&self) -> i64 {
        self.forming_geom_rev
    }

    fn sync_properties(&mut self) {
        self.bar_count = self.series.len() as i64;
        self.revision = self.series.revision().as_u64() as i64;
        self.last_price = self.series.last_close().unwrap_or(0.0);
        self.has_forming = self.series.has_forming();
        if let Some(ext) = self.series.extents() {
            self.first_time = ext.first_time;
            self.last_time = ext.last_time;
            self.low = ext.low;
            self.high = ext.high;
        } else {
            self.first_time = 0;
            self.last_time = 0;
            self.low = 0.0;
            self.high = 0.0;
        }
    }
}

impl Default for BarSeriesRust {
    fn default() -> Self {
        Self::new("DEFAULT", "1m", 500_000)
    }
}

impl ffi::BarSeries {
    pub fn set_viewport(
        mut self: Pin<&mut Self>,
        first_bar: i64,
        last_bar: i64,
        low: f64,
        high: f64,
    ) {
        self.as_mut()
            .rust_mut()
            .get_mut()
            .set_viewport(first_bar, last_bar, low, high);
    }

    pub fn set_surface(mut self: Pin<&mut Self>, width_px: f32, height_px: f32) {
        self.as_mut()
            .rust_mut()
            .get_mut()
            .set_surface(width_px, height_px);
    }

    pub fn vertex_ptr(&self) -> *const ffi::BarVertex {
        self.rust().vertex_ptr()
    }

    pub fn vertex_len(&self) -> usize {
        self.rust().vertex_len()
    }

    pub fn geometry_revision(&self) -> i64 {
        self.rust().geometry_revision()
    }

    pub fn rebuild_geometry(mut self: Pin<&mut Self>) {
        self.as_mut().rust_mut().get_mut().rebuild_geometry();
    }

    pub fn rebuild_split_geometry(mut self: Pin<&mut Self>) {
        self.as_mut().rust_mut().get_mut().rebuild_split_geometry();
    }

    pub fn completed_vertex_ptr(&self) -> *const ffi::BarVertex {
        self.rust().completed_vertex_ptr()
    }

    pub fn completed_vertex_len(&self) -> usize {
        self.rust().completed_vertex_len()
    }

    pub fn completed_geometry_revision(&self) -> i64 {
        self.rust().completed_geometry_revision()
    }

    pub fn forming_vertex_ptr(&self) -> *const ffi::BarVertex {
        self.rust().forming_vertex_ptr()
    }

    pub fn forming_vertex_len(&self) -> usize {
        self.rust().forming_vertex_len()
    }

    pub fn forming_geometry_revision(&self) -> i64 {
        self.rust().forming_geometry_revision()
    }

    pub fn load_history(mut self: Pin<&mut Self>, bars: BarColumns) -> Result<(), SeriesError> {
        self.as_mut().rust_mut().get_mut().load_history(bars)
    }

    pub fn ingest_completed(
        mut self: Pin<&mut Self>,
        batch: BarColumns,
    ) -> Result<(), SeriesError> {
        self.as_mut().rust_mut().get_mut().ingest_completed(batch)
    }

    pub fn ingest_forming(mut self: Pin<&mut Self>, bar: BarColumns) -> Result<(), SeriesError> {
        self.as_mut().rust_mut().get_mut().ingest_forming(bar)
    }

    #[allow(clippy::suboptimal_flops)]
    pub fn load_history_sample(mut self: Pin<&mut Self>, count: i64) -> bool {
        if count <= 0 {
            return false;
        }
        let n = count as usize;
        let mut time = Vec::with_capacity(n);
        let mut open = Vec::with_capacity(n);
        let mut high = Vec::with_capacity(n);
        let mut low = Vec::with_capacity(n);
        let mut close = Vec::with_capacity(n);

        for i in 0..n {
            let t = 1_000_000_000i64 + (i as i64) * 60;
            let o = 100.0 + (i as f64) * 0.5;
            let h = o + 2.0;
            let l = o - 1.5;
            let c = if i % 2 == 0 { o + 1.0 } else { o - 0.5 };
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
        self.as_mut()
            .rust_mut()
            .get_mut()
            .load_history(bars)
            .is_ok()
    }

    pub fn load_history_parquet(mut self: Pin<&mut Self>, path: &QString) -> bool {
        let p_str = path.to_string();
        let p = std::path::Path::new(&p_str);
        match q_io::parquet::read_bar_files(&[p], None) {
            Ok(frame) => self
                .as_mut()
                .rust_mut()
                .get_mut()
                .load_history(frame.bars().clone())
                .is_ok(),
            Err(_) => false,
        }
    }

    pub fn ingest_completed_bar(
        mut self: Pin<&mut Self>,
        time: i64,
        open: f64,
        high: f64,
        low: f64,
        close: f64,
        volume: f64,
    ) -> bool {
        let real_vol = if self
            .as_ref()
            .rust()
            .series
            .completed()
            .volume_set()
            .real_volume
        {
            Some(vec![volume as i64])
        } else {
            None
        };
        let tick_vol = if self
            .as_ref()
            .rust()
            .series
            .completed()
            .volume_set()
            .tick_volume
        {
            Some(vec![volume as i64])
        } else {
            None
        };
        let label = self.as_ref().rust().series.completed().label();
        let bars = BarColumns {
            time: vec![time],
            open: vec![open],
            high: vec![high],
            low: vec![low],
            close: vec![close],
            tick_volume: tick_vol,
            spread: None,
            real_volume: real_vol,
            label,
        };
        self.as_mut()
            .rust_mut()
            .get_mut()
            .ingest_completed(bars)
            .is_ok()
    }

    pub fn ingest_forming_bar(
        mut self: Pin<&mut Self>,
        time: i64,
        open: f64,
        high: f64,
        low: f64,
        close: f64,
        volume: f64,
    ) -> bool {
        let real_vol = if self
            .as_ref()
            .rust()
            .series
            .completed()
            .volume_set()
            .real_volume
        {
            Some(vec![volume as i64])
        } else {
            None
        };
        let tick_vol = if self
            .as_ref()
            .rust()
            .series
            .completed()
            .volume_set()
            .tick_volume
        {
            Some(vec![volume as i64])
        } else {
            None
        };
        let label = self.as_ref().rust().series.completed().label();
        let bars = BarColumns {
            time: vec![time],
            open: vec![open],
            high: vec![high],
            low: vec![low],
            close: vec![close],
            tick_volume: tick_vol,
            spread: None,
            real_volume: real_vol,
            label,
        };
        self.as_mut()
            .rust_mut()
            .get_mut()
            .ingest_forming(bars)
            .is_ok()
    }

    pub fn clear_forming_bar(mut self: Pin<&mut Self>) {
        self.as_mut().rust_mut().get_mut().clear_forming();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_bars(times: &[i64], close_val: f64) -> BarColumns {
        let n = times.len();
        BarColumns {
            time: times.to_vec(),
            open: vec![close_val; n],
            high: vec![close_val + 5.0; n],
            low: vec![close_val - 5.0; n],
            close: vec![close_val; n],
            tick_volume: None,
            spread: None,
            real_volume: None,
            label: TimeLabel::Utc,
        }
    }

    #[test]
    fn test_headless_bar_series_lifecycle() {
        let mut bs = BarSeriesRust::new("TEST", "1m", 100);
        assert_eq!(bs.bar_count, 0);
        assert_eq!(bs.revision, 0);
        assert_eq!(bs.last_price.to_bits(), 0.0f64.to_bits());
        assert!(!bs.has_forming);
        assert_eq!(bs.first_time, 0);
        assert_eq!(bs.last_time, 0);
        assert_eq!(bs.geometry_revision(), 0);
        assert_eq!(bs.vertex_len(), 0);
        assert!(bs.vertex_ptr().is_null());

        // 1. load_history
        let times: Vec<i64> = (1..=10).map(|i| i * 60).collect();
        bs.load_history(make_test_bars(&times, 100.0)).unwrap();
        assert_eq!(bs.bar_count, 10);
        assert_eq!(bs.revision, 1);
        assert_eq!(bs.last_price.to_bits(), 100.0f64.to_bits());
        assert!(!bs.has_forming);
        assert_eq!(bs.first_time, 60);
        assert_eq!(bs.last_time, 600);
        assert_eq!(bs.low.to_bits(), 95.0f64.to_bits());
        assert_eq!(bs.high.to_bits(), 105.0f64.to_bits());

        // 2. ingest_completed
        let next_times: Vec<i64> = vec![660, 720];
        bs.ingest_completed(make_test_bars(&next_times, 110.0))
            .unwrap();
        assert_eq!(bs.bar_count, 12);
        assert_eq!(bs.revision, 2);
        assert_eq!(bs.last_price.to_bits(), 110.0f64.to_bits());
        assert!(!bs.has_forming);
        assert_eq!(bs.last_time, 720);
        assert_eq!(bs.high.to_bits(), 115.0f64.to_bits());

        // 3. ingest_forming
        bs.ingest_forming(make_test_bars(&[780], 120.0)).unwrap();
        assert_eq!(bs.bar_count, 12); // Completed count unchanged
        assert!(bs.has_forming);
        assert_eq!(bs.revision, 3);
        assert_eq!(bs.last_price.to_bits(), 120.0f64.to_bits());
        assert_eq!(bs.last_time, 780);
        assert_eq!(bs.high.to_bits(), 125.0f64.to_bits());

        // 4. rebuild_geometry
        bs.set_viewport(0, 15, 90.0, 130.0);
        bs.set_surface(300.0, 200.0);
        bs.rebuild_geometry();

        assert_eq!(bs.geometry_revision(), 3);
        // 12 completed bars + 1 forming bar = 13 buckets -> 13 * 12 = 156 vertices
        assert_eq!(bs.vertex_len(), 156);
        assert!(!bs.vertex_ptr().is_null());

        // Last bucket's vertices must be flagged forming = 1.0
        unsafe {
            let ptr = bs.vertex_ptr();
            let last_wick_forming = (*ptr.add(156 - 12)).forming;
            assert_eq!(last_wick_forming.to_bits(), 1.0f32.to_bits());
            let first_wick_forming = (*ptr).forming;
            assert_eq!(first_wick_forming.to_bits(), 0.0f32.to_bits());
        }
    }

    unsafe fn vertex_bits_at(ptr: *const ffi::BarVertex, index: usize) -> (u32, u32, u32, u32) {
        let v = &*ptr.add(index);
        (
            v.x.to_bits(),
            v.y.to_bits(),
            v.direction.to_bits(),
            v.forming.to_bits(),
        )
    }

    fn assert_split_matches_combined(bs: &BarSeriesRust) {
        let combined_len = bs.vertex_len();
        let split_len = bs.completed_vertex_len() + bs.forming_vertex_len();
        assert_eq!(combined_len, split_len);

        unsafe {
            let combined_ptr = bs.vertex_ptr();
            let completed_ptr = bs.completed_vertex_ptr();
            let forming_ptr = bs.forming_vertex_ptr();
            let completed_len = bs.completed_vertex_len();
            let forming_len = bs.forming_vertex_len();

            for i in 0..completed_len {
                assert_eq!(
                    vertex_bits_at(combined_ptr, i),
                    vertex_bits_at(completed_ptr, i)
                );
            }
            for i in 0..forming_len {
                assert_eq!(
                    vertex_bits_at(combined_ptr, completed_len + i),
                    vertex_bits_at(forming_ptr, i)
                );
            }
        }
    }

    #[test]
    fn test_split_geometry_matches_combined_and_forming_tick_is_stable() {
        let mut bs = BarSeriesRust::new("TEST", "1m", 100);
        let times: Vec<i64> = (1..=10).map(|i| i * 60).collect();
        bs.load_history(make_test_bars(&times, 100.0)).unwrap();
        bs.ingest_forming(make_test_bars(&[660], 110.0)).unwrap();
        bs.set_viewport(0, 11, 90.0, 130.0);
        bs.set_surface(300.0, 200.0);

        bs.rebuild_geometry();
        bs.rebuild_split_geometry();
        assert_split_matches_combined(&bs);

        let completed_ptr = bs.completed_vertex_ptr();
        let completed_rev = bs.completed_geometry_revision();
        let completed_len = bs.completed_vertex_len();
        let completed_bucket_cap = bs.completed_geom_vertices.capacity();

        bs.ingest_forming(make_test_bars(&[660], 115.0)).unwrap();
        bs.rebuild_split_geometry();

        assert_eq!(bs.completed_vertex_ptr(), completed_ptr);
        assert_eq!(bs.completed_geometry_revision(), completed_rev);
        assert_eq!(bs.completed_vertex_len(), completed_len);
        assert_eq!(bs.completed_geom_vertices.capacity(), completed_bucket_cap);
        assert!(bs.forming_geometry_revision() > completed_rev);
        assert_eq!(bs.forming_vertex_len(), 12);

        bs.rebuild_geometry();
        assert_split_matches_combined(&bs);
    }

    #[test]
    fn test_split_geometry_offscreen_forming_is_unchanged() {
        let mut bs = BarSeriesRust::new("TEST", "1m", 100);
        let times: Vec<i64> = (1..=10).map(|i| i * 60).collect();
        bs.load_history(make_test_bars(&times, 100.0)).unwrap();
        bs.ingest_forming(make_test_bars(&[660], 110.0)).unwrap();
        bs.set_viewport(0, 10, 90.0, 130.0);
        bs.set_surface(300.0, 200.0);
        bs.rebuild_split_geometry();

        let completed_rev = bs.completed_geometry_revision();
        let forming_rev = bs.forming_geometry_revision();
        assert_eq!(bs.forming_vertex_len(), 0);
        assert!(bs.forming_vertex_ptr().is_null());

        bs.ingest_forming(make_test_bars(&[660], 120.0)).unwrap();
        bs.rebuild_split_geometry();
        assert_eq!(bs.completed_geometry_revision(), completed_rev);
        assert_eq!(bs.forming_geometry_revision(), forming_rev);
        assert_eq!(bs.forming_vertex_len(), 0);
    }

    #[test]
    fn test_split_geometry_completed_append_and_viewport_invalidate_completed_only() {
        let mut bs = BarSeriesRust::new("TEST", "1m", 100);
        let times: Vec<i64> = (1..=10).map(|i| i * 60).collect();
        bs.load_history(make_test_bars(&times, 100.0)).unwrap();
        bs.ingest_forming(make_test_bars(&[660], 110.0)).unwrap();
        bs.set_viewport(0, 11, 90.0, 130.0);
        bs.set_surface(300.0, 200.0);
        bs.rebuild_split_geometry();

        let completed_rev = bs.completed_geometry_revision();
        bs.ingest_completed(make_test_bars(&[720], 120.0)).unwrap();
        bs.rebuild_split_geometry();
        assert!(bs.completed_geometry_revision() > completed_rev);
        assert_eq!(bs.completed_vertex_len(), 11 * 12);
        assert_eq!(bs.forming_vertex_len(), 0);

        let completed_rev = bs.completed_geometry_revision();
        bs.set_viewport(0, 12, 90.0, 130.0);
        bs.ingest_forming(make_test_bars(&[780], 125.0)).unwrap();
        bs.rebuild_split_geometry();
        assert!(bs.completed_geometry_revision() > completed_rev);
        assert_eq!(bs.forming_vertex_len(), 12);
    }

    #[test]
    fn test_split_geometry_forming_clear_and_reappear() {
        let mut bs = BarSeriesRust::new("TEST", "1m", 100);
        let times: Vec<i64> = (1..=10).map(|i| i * 60).collect();
        bs.load_history(make_test_bars(&times, 100.0)).unwrap();
        bs.ingest_forming(make_test_bars(&[660], 110.0)).unwrap();
        bs.set_viewport(0, 11, 90.0, 130.0);
        bs.set_surface(300.0, 200.0);
        bs.rebuild_split_geometry();
        assert_eq!(bs.forming_vertex_len(), 12);

        let forming_rev = bs.forming_geometry_revision();
        bs.clear_forming();
        bs.rebuild_split_geometry();
        assert!(bs.forming_geometry_revision() > forming_rev);
        assert_eq!(bs.forming_vertex_len(), 0);

        let completed_rev = bs.completed_geometry_revision();
        bs.ingest_forming(make_test_bars(&[660], 110.0)).unwrap();
        bs.rebuild_split_geometry();
        assert_eq!(bs.completed_geometry_revision(), completed_rev);
        assert_eq!(bs.forming_vertex_len(), 12);
    }

    #[test]
    fn test_split_geometry_empty_and_zero_size_views() {
        let mut bs = BarSeriesRust::new("TEST", "1m", 100);
        bs.set_viewport(0, 10, 0.0, 100.0);
        bs.set_surface(300.0, 200.0);
        bs.rebuild_split_geometry();
        assert_eq!(bs.completed_vertex_len(), 0);
        assert_eq!(bs.forming_vertex_len(), 0);

        let times: Vec<i64> = (1..=5).map(|i| i * 60).collect();
        bs.load_history(make_test_bars(&times, 100.0)).unwrap();
        bs.set_surface(0.0, 0.0);
        bs.rebuild_split_geometry();
        assert_eq!(bs.completed_vertex_len(), 0);
        assert_eq!(bs.forming_vertex_len(), 0);
    }
}
