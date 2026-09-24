//! Vertex packing: converts downsampled buckets into GPU vertex data in surface coordinates.

use crate::lod::Bucket;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Viewport {
    pub first_bar: usize,
    pub last_bar: usize,
    pub low: f64,
    pub high: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Surface {
    pub width_px: f32,
    pub height_px: f32,
}

/// One vertex: surface coordinates, plus 1.0 for an up bucket and -1.0 for a
/// down one, and 1.0 on the forming bucket's vertices.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BarVertex {
    pub x: f32,
    pub y: f32,
    pub direction: f32,
    pub forming: f32,
}

/// Appends 12 vertices for one `bucket` to `out`.
pub fn pack_bucket(
    bucket: &Bucket,
    bucket_index: usize,
    bucket_count: usize,
    view: Viewport,
    surface: Surface,
    out: &mut Vec<BarVertex>,
) {
    let price_range = view.high - view.low;
    let price_to_y = |p: f64| -> f32 {
        if !price_range.is_finite() || price_range <= 0.0 {
            surface.height_px * 0.5
        } else {
            let norm = (view.high - p) / price_range;
            (norm * surface.height_px as f64) as f32
        }
    };

    let total_bars = if view.last_bar > view.first_bar {
        (view.last_bar - view.first_bar) as f32
    } else {
        0.0
    };

    let (x_center, col_width) = if total_bars > 0.0 {
        let bar_center = (bucket.start as f32 + bucket.end as f32) * 0.5;
        let norm_x = (bar_center - view.first_bar as f32) / total_bars;
        let bar_span = (bucket.end - bucket.start) as f32;
        let width = (bar_span / total_bars) * surface.width_px;
        (norm_x * surface.width_px, width.max(1.0))
    } else {
        let width = (surface.width_px / bucket_count as f32).max(1.0);
        let center = (bucket_index as f32 + 0.5) * width;
        (center, width)
    };

    let body_width = (col_width * 0.8).max(1.0);
    let half_body = body_width * 0.5;
    let body_left = x_center - half_body;
    let body_right = x_center + half_body;

    let wick_width = 1.0f32;
    let half_wick = wick_width * 0.5;
    let wick_left = x_center - half_wick;
    let wick_right = x_center + half_wick;

    let y_open = price_to_y(bucket.open);
    let y_close = price_to_y(bucket.close);
    let y_high = price_to_y(bucket.high);
    let y_low = price_to_y(bucket.low);

    let body_top = y_open.min(y_close);
    let mut body_bottom = y_open.max(y_close);
    if body_bottom - body_top < 1.0 {
        body_bottom = body_top + 1.0;
    }

    let wick_top = y_high.min(y_low);
    let mut wick_bottom = y_high.max(y_low);
    if wick_bottom - wick_top < 1.0 {
        wick_bottom = wick_top + 1.0;
    }

    let direction = if bucket.close >= bucket.open {
        1.0f32
    } else {
        -1.0f32
    };
    let is_forming =
        bucket.forming && (bucket.start < view.last_bar && bucket.end > view.first_bar);
    let forming = if is_forming { 1.0f32 } else { 0.0f32 };

    // 1. Wick quad (2 triangles = 6 vertices)
    out.push(BarVertex {
        x: wick_left,
        y: wick_top,
        direction,
        forming,
    });
    out.push(BarVertex {
        x: wick_right,
        y: wick_top,
        direction,
        forming,
    });
    out.push(BarVertex {
        x: wick_left,
        y: wick_bottom,
        direction,
        forming,
    });

    out.push(BarVertex {
        x: wick_left,
        y: wick_bottom,
        direction,
        forming,
    });
    out.push(BarVertex {
        x: wick_right,
        y: wick_top,
        direction,
        forming,
    });
    out.push(BarVertex {
        x: wick_right,
        y: wick_bottom,
        direction,
        forming,
    });

    // 2. Body quad (2 triangles = 6 vertices)
    out.push(BarVertex {
        x: body_left,
        y: body_top,
        direction,
        forming,
    });
    out.push(BarVertex {
        x: body_right,
        y: body_top,
        direction,
        forming,
    });
    out.push(BarVertex {
        x: body_left,
        y: body_bottom,
        direction,
        forming,
    });

    out.push(BarVertex {
        x: body_left,
        y: body_bottom,
        direction,
        forming,
    });
    out.push(BarVertex {
        x: body_right,
        y: body_top,
        direction,
        forming,
    });
    out.push(BarVertex {
        x: body_right,
        y: body_bottom,
        direction,
        forming,
    });
}

/// Packs bodies and wicks for `buckets` into `out`, reusing its capacity.
pub fn pack(buckets: &[Bucket], view: Viewport, surface: Surface, out: &mut Vec<BarVertex>) {
    out.clear();

    if buckets.is_empty() || surface.width_px <= 0.0 || surface.height_px <= 0.0 {
        return;
    }

    out.reserve(buckets.len() * 12);

    let bucket_count = buckets.len();
    for (i, b) in buckets.iter().enumerate() {
        pack_bucket(b, i, bucket_count, view, surface, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lod::reduce_into;
    use crate::{BarColumns, BarFrame, TimeLabel};

    fn assert_vertex_bits_eq(a: BarVertex, b: BarVertex) {
        assert_eq!(a.x.to_bits(), b.x.to_bits());
        assert_eq!(a.y.to_bits(), b.y.to_bits());
        assert_eq!(a.direction.to_bits(), b.direction.to_bits());
        assert_eq!(a.forming.to_bits(), b.forming.to_bits());
    }

    #[test]
    fn test_two_bucket_viewport_expected_coordinates() {
        let view = Viewport {
            first_bar: 0,
            last_bar: 2,
            low: 0.0,
            high: 100.0,
        };
        let surface = Surface {
            width_px: 200.0,
            height_px: 100.0,
        };

        let buckets = vec![
            // Bucket 0 (rising): open 20, high 80, low 10, close 60
            Bucket {
                start: 0,
                end: 1,
                open: 20.0,
                high: 80.0,
                low: 10.0,
                close: 60.0,
                forming: false,
            },
            // Bucket 1 (falling, forming): open 70, high 90, low 30, close 40
            Bucket {
                start: 1,
                end: 2,
                open: 70.0,
                high: 90.0,
                low: 30.0,
                close: 40.0,
                forming: true,
            },
        ];

        let mut out = Vec::new();
        pack(&buckets, view, surface, &mut out);
        assert_eq!(out.len(), 24);

        // Bucket 0 wick (vertices 0..6):
        // x_center = 50.0, wick_left = 49.5, wick_right = 50.5
        // y_high = 20.0, y_low = 90.0
        assert_vertex_bits_eq(
            out[0],
            BarVertex {
                x: 49.5,
                y: 20.0,
                direction: 1.0,
                forming: 0.0,
            },
        );
        assert_vertex_bits_eq(
            out[1],
            BarVertex {
                x: 50.5,
                y: 20.0,
                direction: 1.0,
                forming: 0.0,
            },
        );
        assert_vertex_bits_eq(
            out[2],
            BarVertex {
                x: 49.5,
                y: 90.0,
                direction: 1.0,
                forming: 0.0,
            },
        );
        assert_vertex_bits_eq(
            out[3],
            BarVertex {
                x: 49.5,
                y: 90.0,
                direction: 1.0,
                forming: 0.0,
            },
        );
        assert_vertex_bits_eq(
            out[4],
            BarVertex {
                x: 50.5,
                y: 20.0,
                direction: 1.0,
                forming: 0.0,
            },
        );
        assert_vertex_bits_eq(
            out[5],
            BarVertex {
                x: 50.5,
                y: 90.0,
                direction: 1.0,
                forming: 0.0,
            },
        );

        // Bucket 0 body (vertices 6..12):
        // body_width = 80.0, body_left = 10.0, body_right = 90.0
        // y_open = 80.0, y_close = 40.0 -> body_top = 40.0, body_bottom = 80.0
        assert_vertex_bits_eq(
            out[6],
            BarVertex {
                x: 10.0,
                y: 40.0,
                direction: 1.0,
                forming: 0.0,
            },
        );
        assert_vertex_bits_eq(
            out[7],
            BarVertex {
                x: 90.0,
                y: 40.0,
                direction: 1.0,
                forming: 0.0,
            },
        );
        assert_vertex_bits_eq(
            out[8],
            BarVertex {
                x: 10.0,
                y: 80.0,
                direction: 1.0,
                forming: 0.0,
            },
        );
        assert_vertex_bits_eq(
            out[9],
            BarVertex {
                x: 10.0,
                y: 80.0,
                direction: 1.0,
                forming: 0.0,
            },
        );
        assert_vertex_bits_eq(
            out[10],
            BarVertex {
                x: 90.0,
                y: 40.0,
                direction: 1.0,
                forming: 0.0,
            },
        );
        assert_vertex_bits_eq(
            out[11],
            BarVertex {
                x: 90.0,
                y: 80.0,
                direction: 1.0,
                forming: 0.0,
            },
        );

        // Bucket 1 (falling, forming):
        // direction is -1.0, forming is 1.0
        for v in &out[12..24] {
            assert_eq!(v.direction.to_bits(), (-1.0f32).to_bits());
            assert_eq!(v.forming.to_bits(), 1.0f32.to_bits());
        }
    }

    #[test]
    fn test_direction_and_forming_flags() {
        let view = Viewport {
            first_bar: 0,
            last_bar: 2,
            low: 0.0,
            high: 100.0,
        };
        let surface = Surface {
            width_px: 200.0,
            height_px: 100.0,
        };

        let buckets = vec![
            Bucket {
                start: 0,
                end: 1,
                open: 10.0,
                high: 20.0,
                low: 5.0,
                close: 15.0,
                forming: false,
            },
            Bucket {
                start: 1,
                end: 2,
                open: 20.0,
                high: 25.0,
                low: 10.0,
                close: 12.0,
                forming: true,
            },
        ];

        let mut out = Vec::new();
        pack(&buckets, view, surface, &mut out);

        for v in &out[0..12] {
            assert_eq!(v.direction.to_bits(), 1.0f32.to_bits());
            assert_eq!(v.forming.to_bits(), 0.0f32.to_bits());
        }
        for v in &out[12..24] {
            assert_eq!(v.direction.to_bits(), (-1.0f32).to_bits());
            assert_eq!(v.forming.to_bits(), 1.0f32.to_bits());
        }

        // Viewport excludes the forming bucket: view.last_bar = 1
        let view_excludes = Viewport {
            first_bar: 0,
            last_bar: 1,
            low: 0.0,
            high: 100.0,
        };
        let mut out2 = Vec::new();
        pack(&buckets, view_excludes, surface, &mut out2);
        for v in &out2[12..24] {
            assert_eq!(v.forming.to_bits(), 0.0f32.to_bits());
        }
    }

    #[test]
    fn test_packing_twice_gives_identical_output() {
        let view = Viewport {
            first_bar: 0,
            last_bar: 5,
            low: 10.0,
            high: 50.0,
        };
        let surface = Surface {
            width_px: 500.0,
            height_px: 300.0,
        };
        let buckets = vec![
            Bucket {
                start: 0,
                end: 2,
                open: 20.0,
                high: 40.0,
                low: 15.0,
                close: 30.0,
                forming: false,
            },
            Bucket {
                start: 2,
                end: 5,
                open: 30.0,
                high: 45.0,
                low: 25.0,
                close: 35.0,
                forming: true,
            },
        ];

        let mut out1 = Vec::new();
        pack(&buckets, view, surface, &mut out1);

        let mut out2 = Vec::new();
        pack(&buckets, view, surface, &mut out2);

        assert_eq!(out1.len(), out2.len());
        for (v1, v2) in out1.iter().zip(out2.iter()) {
            assert_vertex_bits_eq(*v1, *v2);
        }
    }

    #[test]
    fn test_degenerate_price_range_and_single_bar_viewport() {
        // Zero-height price range
        let view_flat = Viewport {
            first_bar: 0,
            last_bar: 2,
            low: 50.0,
            high: 50.0,
        };
        let surface = Surface {
            width_px: 100.0,
            height_px: 100.0,
        };
        let buckets = vec![Bucket {
            start: 0,
            end: 1,
            open: 50.0,
            high: 50.0,
            low: 50.0,
            close: 50.0,
            forming: false,
        }];

        let mut out = Vec::new();
        pack(&buckets, view_flat, surface, &mut out);
        assert_eq!(out.len(), 12);
        for v in &out {
            assert!(v.x.is_finite());
            assert!(v.y.is_finite());
            assert!(!v.x.is_nan());
            assert!(!v.y.is_nan());
        }

        // Single-bar viewport
        let view_single = Viewport {
            first_bar: 5,
            last_bar: 6,
            low: 10.0,
            high: 20.0,
        };
        let single_bucket = vec![Bucket {
            start: 5,
            end: 6,
            open: 12.0,
            high: 18.0,
            low: 11.0,
            close: 16.0,
            forming: false,
        }];
        let mut out_single = Vec::new();
        pack(&single_bucket, view_single, surface, &mut out_single);
        assert_eq!(out_single.len(), 12);
        for v in &out_single {
            assert!(v.x.is_finite());
            assert!(v.y.is_finite());
            assert!(!v.x.is_nan());
            assert!(!v.y.is_nan());
        }

        // Zero-width bar range
        let view_zero_bar = Viewport {
            first_bar: 5,
            last_bar: 5,
            low: 10.0,
            high: 20.0,
        };
        let mut out_zero = Vec::new();
        pack(&single_bucket, view_zero_bar, surface, &mut out_zero);
        assert_eq!(out_zero.len(), 12);
        for v in &out_zero {
            assert!(v.x.is_finite());
            assert!(v.y.is_finite());
            assert!(!v.x.is_nan());
            assert!(!v.y.is_nan());
        }
    }

    fn assert_vertices_equal(a: &[BarVertex], b: &[BarVertex]) {
        assert_eq!(a.len(), b.len());
        for (v1, v2) in a.iter().zip(b.iter()) {
            assert_vertex_bits_eq(*v1, *v2);
        }
    }

    fn pack_split(buckets: &[Bucket], view: Viewport, surface: Surface) -> Vec<BarVertex> {
        let completed = buckets.iter().filter(|b| !b.forming).collect::<Vec<_>>();
        let forming = buckets.iter().find(|b| b.forming);

        let mut out = Vec::new();
        if !completed.is_empty() {
            let count = completed.len();
            for (i, b) in completed.iter().enumerate() {
                pack_bucket(b, i, count, view, surface, &mut out);
            }
        }
        if let Some(b) = forming {
            pack_bucket(b, 0, 1, view, surface, &mut out);
        }
        out
    }

    #[allow(clippy::suboptimal_flops)]
    fn make_frame(n: usize, flat_price: bool) -> BarFrame {
        let mut open = Vec::with_capacity(n);
        let mut high = Vec::with_capacity(n);
        let mut low = Vec::with_capacity(n);
        let mut close = Vec::with_capacity(n);
        let mut time = Vec::with_capacity(n);
        for i in 0..n {
            time.push((i as i64 + 1) * 60);
            if flat_price {
                open.push(50.0);
                high.push(50.0);
                low.push(50.0);
                close.push(50.0);
            } else {
                let o = 100.0 + (i as f64) * 0.5;
                open.push(o);
                high.push(o + 2.0);
                low.push(o - 1.5);
                close.push(if i % 2 == 0 { o + 1.0 } else { o - 0.5 });
            }
        }
        BarFrame::try_new(BarColumns {
            time,
            open,
            high,
            low,
            close,
            tick_volume: None,
            spread: None,
            real_volume: None,
            label: TimeLabel::Utc,
        })
        .expect("valid frame")
    }

    #[test]
    fn test_split_pack_matches_legacy_rising_falling_and_forming() {
        let view = Viewport {
            first_bar: 0,
            last_bar: 2,
            low: 0.0,
            high: 100.0,
        };
        let surface = Surface {
            width_px: 200.0,
            height_px: 100.0,
        };
        let buckets = vec![
            Bucket {
                start: 0,
                end: 1,
                open: 20.0,
                high: 80.0,
                low: 10.0,
                close: 60.0,
                forming: false,
            },
            Bucket {
                start: 1,
                end: 2,
                open: 70.0,
                high: 90.0,
                low: 30.0,
                close: 40.0,
                forming: true,
            },
        ];

        let mut legacy = Vec::new();
        pack(&buckets, view, surface, &mut legacy);
        let split = pack_split(&buckets, view, surface);
        assert_vertices_equal(&legacy, &split);
    }

    #[test]
    fn test_split_pack_matches_legacy_flat_price_and_single_bar() {
        let surface = Surface {
            width_px: 100.0,
            height_px: 100.0,
        };

        let view_flat = Viewport {
            first_bar: 0,
            last_bar: 1,
            low: 50.0,
            high: 50.0,
        };
        let flat_bucket = vec![Bucket {
            start: 0,
            end: 1,
            open: 50.0,
            high: 50.0,
            low: 50.0,
            close: 50.0,
            forming: false,
        }];
        let mut legacy_flat = Vec::new();
        pack(&flat_bucket, view_flat, surface, &mut legacy_flat);
        assert_vertices_equal(&legacy_flat, &pack_split(&flat_bucket, view_flat, surface));

        let view_single = Viewport {
            first_bar: 5,
            last_bar: 6,
            low: 10.0,
            high: 20.0,
        };
        let single_bucket = vec![Bucket {
            start: 5,
            end: 6,
            open: 12.0,
            high: 18.0,
            low: 11.0,
            close: 16.0,
            forming: false,
        }];
        let mut legacy_single = Vec::new();
        pack(&single_bucket, view_single, surface, &mut legacy_single);
        assert_vertices_equal(
            &legacy_single,
            &pack_split(&single_bucket, view_single, surface),
        );
    }

    #[test]
    fn test_split_pack_matches_legacy_lod_buckets_with_forming() {
        let frame = make_frame(100, false);
        let view = Viewport {
            first_bar: 0,
            last_bar: 100,
            low: 90.0,
            high: 160.0,
        };
        let surface = Surface {
            width_px: 50.0,
            height_px: 300.0,
        };

        let mut completed_buckets = Vec::new();
        reduce_into(&frame, 0..99, 50, &mut completed_buckets).expect("reduce");

        let forming_bucket = Bucket {
            start: 99,
            end: 100,
            open: frame.open()[99],
            high: frame.high()[99],
            low: frame.low()[99],
            close: frame.close()[99],
            forming: true,
        };

        let mut combined = completed_buckets.clone();
        combined.push(forming_bucket);

        let mut legacy = Vec::new();
        pack(&combined, view, surface, &mut legacy);
        let split = pack_split(&combined, view, surface);
        assert_vertices_equal(&legacy, &split);
        assert_eq!(legacy.len(), combined.len() * 12);
    }
}
