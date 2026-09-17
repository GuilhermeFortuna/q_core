#![allow(clippy::float_cmp)]
//! CXX-Qt bridge and Rust implementation for LiveBarSeries projected to Qt/QML.

use std::pin::Pin;

use cxx_qt::CxxQtType;
use cxx_qt_lib::QString;
use q_buffers::frame::{BarColumns, TimeLabel, VolumeSet};
use q_buffers::geometry::{pack as pack_geometry, BarVertex as GeomBarVertex, Surface, Viewport};
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

        fn vertex_ptr(self: &BarSeries) -> *const BarVertex;

        fn vertex_len(self: &BarSeries) -> usize;

        fn geometry_revision(self: &BarSeries) -> i64;

        fn rebuild_geometry(self: Pin<&mut BarSeries>);
    }
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
        }
    }

    pub fn load_history(&mut self, bars: BarColumns) -> Result<(), SeriesError> {
        self.series.load_history(bars)?;
        self.sync_properties();
        Ok(())
    }

    pub fn ingest_completed(&mut self, batch: BarColumns) -> Result<(), SeriesError> {
        self.series.append_completed(batch)?;
        self.sync_properties();
        Ok(())
    }

    pub fn ingest_forming(&mut self, bar: BarColumns) -> Result<(), SeriesError> {
        self.series.set_forming(bar)?;
        self.sync_properties();
        Ok(())
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
}
