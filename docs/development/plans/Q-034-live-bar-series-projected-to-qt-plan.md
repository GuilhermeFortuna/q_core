# Q-034 implementation plan: Live bar series projected to Qt

**Status:** authoritative in the [Q project board](https://github.com/users/GuilhermeFortuna/projects/2)  
**Specification:** [`../specs/Q-034-live-bar-series-projected-to-qt-spec.md`](../specs/Q-034-live-bar-series-projected-to-qt-spec.md)  
**Depends on:** Q-033

## Current-system context

`crates/q-buffers/src/window.rs` holds `RollingBarWindow`, the evaluation
window: `ingest_completed` sorts the incoming batch, concatenates, keeps the
last of each duplicated time, sorts again and trims to `bound`; `set_forming`
and `clear_forming` hold one forming bar apart; `completed()` and `forming()`
return `&BarFrame`. Its semantics are Q-025's, proven equal to the backend's
pandas window by the parity gate, and any change to them changes evaluation.

`crates/q-buffers/src/frame.rs` gives `BarColumns`, `BarFrame` (`try_new`,
`len`, `time`, `open`, `high`, `low`, `close`, optional `tick_volume`,
`spread`, `real_volume`, `push_column`, `column`, `schema`), `VolumeSet`,
`TimeLabel` and `FrameError`. `column.rs` gives `Column`, `ColumnType` and
`Bitmap`. Q-033 adds `q-io` with `decode_bar_batches`, `read_bar_files`,
`bar_file_rows` and `digest_file`.

`crates/q-qt/src/lib.rs` is a 30-line `#[cxx_qt::bridge]` exposing one
`CoreInfo` QObject with `version` and `contracts_rev` `QString` properties,
built by `CxxQtBuilder::new().file("src/lib.rs").build()`. The crate is
`crate-type = ["staticlib", "rlib"]` and depends on `q-engine`, `q-io`,
`q-buffers`, `q-indicators`, `cxx 1.0.142`, `cxx-qt 0.10.0` and
`cxx-qt-lib 0.10.0`. `make qt-test` compiles `crates/q-qt/tests/harness.cpp`
against the static library and the `qt_minimal` Qt 6 and runs it; it links
`Qt6Core` only.

`q_terminal` consumes this crate as a git dependency pinned to the tag
`v2026.09.12` (`q_terminal/Cargo.toml`), while `q_core` is at
`v2026.09.15.2`. The pin moves when a terminal task first needs a newer tag;
this task only produces the tag.

`q_terminal/cpp/README.md` states the C++ rule this task is designed against:
scene-graph nodes move buffers and compute nothing.

The gap is that `q_core` has no display-side series, no level-of-detail, and no
way to hand bytes to a Qt renderer.

## Interfaces produced

```rust
// crates/q-buffers/src/series.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Revision(u64);

/// Bars changed since a caller's revision. `TooOld` means "refresh everything".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirtyRange { None, Bars { start: usize, end: usize }, TooOld }

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SeriesExtents { pub first_time: i64, pub last_time: i64, pub low: f64, pub high: f64 }

/// Growing display series for one symbol and timeframe, with a forming bar held
/// apart. Distinct from `RollingBarWindow`: this one refuses an out-of-order
/// batch instead of sorting it.
pub struct LiveBarSeries { /* capacity, completed: BarColumns, forming: Option<BarColumns>, revision, dirty */ }

impl LiveBarSeries {
    pub fn new(capacity: usize, volumes: VolumeSet, label: TimeLabel) -> Result<Self, FrameError>;
    pub fn load_history(&mut self, bars: BarColumns) -> Result<(), SeriesError>;
    pub fn append_completed(&mut self, batch: BarColumns) -> Result<(), SeriesError>;
    pub fn set_forming(&mut self, bar: BarColumns) -> Result<(), SeriesError>;
    pub fn clear_forming(&mut self);
    pub fn len(&self) -> usize;
    pub fn is_empty(&self) -> bool;
    pub fn has_forming(&self) -> bool;
    pub fn last_close(&self) -> Option<f64>;
    pub fn extents(&self) -> Option<SeriesExtents>;
    pub fn revision(&self) -> Revision;
    pub fn dirty_since(&self, since: Revision) -> DirtyRange;
    pub fn completed(&self) -> &BarFrame;
    pub fn forming(&self) -> Option<&BarFrame>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SeriesError { NotAscending { at: usize }, BeforeLast { at: usize }, Frame(FrameError), Empty }

// crates/q-buffers/src/lod.rs
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bucket {
    pub start: usize, pub end: usize,          // half-open bar range
    pub open: f64, pub high: f64, pub low: f64, pub close: f64,
}

/// Exact reduction of `range` to at most `columns` buckets. Boundaries come only
/// from the range length and the column count, so the result is stable.
pub fn reduce(frame: &BarFrame, range: Range<usize>, columns: usize)
    -> Result<Vec<Bucket>, SeriesError>;

/// Reduces into a caller-owned buffer, so a per-frame call allocates nothing.
pub fn reduce_into(frame: &BarFrame, range: Range<usize>, columns: usize, out: &mut Vec<Bucket>)
    -> Result<(), SeriesError>;

// crates/q-buffers/src/geometry.rs
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Viewport { pub first_bar: usize, pub last_bar: usize, pub low: f64, pub high: f64 }
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Surface { pub width_px: f32, pub height_px: f32 }

/// One vertex: surface coordinates, plus 1.0 for an up bucket and -1.0 for a
/// down one, and 1.0 on the forming bucket's vertices.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BarVertex { pub x: f32, pub y: f32, pub direction: f32, pub forming: f32 }

/// Packs bodies and wicks for `buckets` into `out`, reusing its capacity.
pub fn pack(buckets: &[Bucket], view: Viewport, surface: Surface, out: &mut Vec<BarVertex>);

// crates/q-qt/src/bar_series.rs  (cxx_qt bridge)
// QObject `BarSeries`, QML-exposed properties:
//   symbol, timeframe (QString); bar_count, revision (i64); last_price (f64);
//   has_forming (bool); first_time, last_time (i64); low, high (f64)
// Invokables:
//   set_viewport(first_bar: i64, last_bar: i64, low: f64, high: f64)
//   set_surface(width_px: f32, height_px: f32)
// C++-facing, declared in the bridge's `extern "RustQt"` block:
//   fn vertex_ptr(self: &BarSeries) -> *const BarVertex;   // valid until the
//   fn vertex_len(self: &BarSeries) -> usize;              // next mutation
//   fn geometry_revision(self: &BarSeries) -> i64;
//   fn rebuild_geometry(self: Pin<&mut BarSeries>);        // called on the render thread's sync step
// Rust-facing, no Qt loop required:
//   pub fn load_history(&mut self, bars: BarColumns) -> Result<(), SeriesError>;
//   pub fn ingest_completed(&mut self, batch: BarColumns) -> Result<(), SeriesError>;
//   pub fn ingest_forming(&mut self, bar: BarColumns) -> Result<(), SeriesError>;
```

## Implementation decisions

- **A new `LiveBarSeries` rather than an option on `RollingBarWindow`.** The
  window sorts and deduplicates a batch because the backend's pandas window
  does, and the parity gate proves it. A display series must instead refuse a
  batch that arrives out of order, because out-of-order arrival on the stream is
  a gap that Q-035 must handle by re-snapshotting, not something to paper over
  by sorting. One type cannot hold both rules.

- **Capacity is enforced by dropping from the front of the column vectors, not
  by a ring buffer.** A ring would make every geometry pass handle a wrap, on
  the hot path, to save an occasional memmove of a bounded buffer. The drop
  happens once per capacity overflow, and `Vec::drain` on a 500,000-element
  f64 column is far below the frame budget.

- **The dirty range is a single coalesced range plus a horizon, not a log.**
  Consumers are renderers: they ask "what changed since I drew", and a union
  range is the answer they can act on. The horizon is the last history load or
  capacity drop, past which the answer is `TooOld` and the consumer redraws
  everything — the same code path as first draw, which §4.2 already establishes
  as the pattern for the stream.

- **Bucket boundaries are computed from the range length and column count with
  integer arithmetic, never from accumulated float steps.** A float step
  accumulates error across 2,000 columns and would make the same viewport give
  different buckets depending on how it was reached. Integer boundaries are what
  make the determinism test possible.

- **Extremes are folded exactly, and NaN propagates.** `f64::max` ignores NaN,
  which would silently turn a missing high into a number. The fold returns NaN as
  soon as it sees one, matching Q-029's aggregation rule, so a defective payload
  is visible rather than plausible.

- **Geometry is produced in surface pixel coordinates, not normalised device
  coordinates.** The node draws into a Qt item whose size Qt owns; giving the
  node pixels means it applies only Qt's own transform and computes nothing,
  which is the rule `cpp/README.md` states. It also makes the vertex values
  directly assertable in a unit test.

- **`BarVertex` is `#[repr(C)]` with four `f32`s and no padding.** It is read by
  C++ and uploaded as an interleaved vertex buffer; a layout the Rust compiler
  may reorder cannot be. Four `f32`s also match a single `QSGGeometry` attribute
  set in Q-037 without a repack.

- **The pointer handoff exposes a `*const BarVertex` valid until the next
  mutation, and geometry is rebuilt only in `rebuild_geometry`.** Qt's scene
  graph has exactly one point where the GUI and render threads are synchronised;
  giving the renderer a pointer that is rebuilt anywhere else would be a data
  race. Q-037's node calls `rebuild_geometry` in its sync step and reads the
  pointer in its update step, and the revision tells it whether to re-upload.

- **The buffers behind the pointer are owned by the object and reused.** A
  per-frame allocation is the difference between meeting and missing the 2 ms
  budget at 8,000 buckets; `reduce_into` and `pack` both take an `out` the
  object owns so the steady state allocates nothing.

- **The Qt object's Rust-facing ingestion methods take `BarColumns`, not bytes.**
  Decoding is `q-io`'s (Q-033) and the terminal's transport is Q-035's. Keeping
  this object at the column level is what lets the headless test drive it
  without a socket.

## Ordered implementation

- [x] 1. Work on the branch `Q-034-live-bar-series-projected-to-qt` in `q_core`,
   created from `development` by `./work start`. Confirm Q-033 has merged into
   `development` and that `make check` passes on it before changing anything.
- [x] 2. Write failing tests in `series.rs`: append extends; an equal-time bar
   replaces the last; a batch starting before the last bar gives `BeforeLast`
   naming the position and leaves length and revision unchanged; an internally
   unordered batch gives `NotAscending`; capacity 3 with 5 bars keeps the last
   3; a forming bar replaced 1,000 times leaves `completed()` byte-identical; a
   completed bar at the forming time clears the forming bar; a failed
   `load_history` leaves the previous series intact. Implement `LiveBarSeries`.
   Confirm they pass. Commit.
- [x] 3. Write failing tests for revision and dirty tracking: the counter advances
   on each accepted mutation and not on a refused one; `dirty_since` covers
   exactly the changed bars after an append, a replace and a forming change;
   after `load_history` and after a capacity drop it is `TooOld`; `dirty_since`
   at the current revision is `None`. Implement. Confirm they pass. Commit.
- [x] 4. Write failing tests in `lod.rs`: 1,000 bars into 300 columns gives 300
   buckets whose boundaries match integer division; the same call twice gives
   identical buckets; 100 bars into 300 columns gives 100 single-bar buckets; a
   bucket's high and low equal the max and min of its bars, and a NaN high
   propagates; an empty range gives no buckets; zero columns is an error;
   `reduce_into` on a pre-sized buffer allocates nothing. Implement. Confirm they
   pass. Commit.
- [x] 5. Write failing tests in `geometry.rs`: for a two-bucket viewport at a
   known surface size, every packed vertex has its expected pixel coordinate;
   `direction` is 1.0 for a close above the open and -1.0 below; `forming` is
   1.0 only on the last bucket's vertices when the viewport includes it; packing
   twice gives byte-identical output; a zero-height price range and a
   single-bar viewport both give finite coordinates. Implement `pack`. Confirm
   they pass. Commit.
- [ ] 6. Add the `BarSeries` bridge to `q-qt` with its properties, invokables and
   the four C++-facing functions, delegating every computation to `q-buffers`.
   Write failing Rust tests that drive `load_history`, `ingest_completed` and
   `ingest_forming` with no event loop and assert bar count, extents, last price,
   forming flag, geometry revision and vertex length. Implement. Confirm they
   pass. Commit.
- [ ] 7. Extend `crates/q-qt/tests/harness.cpp`: construct a `BarSeries`, load a
   generated history, set a viewport and surface, call `rebuild_geometry`, read
   the pointer and length, assert the first and last vertices and that the
   revision advances only on mutation. Confirm `make qt-test` passes. Commit.
- [ ] 8. Add `bench-bar-geometry` to the `Makefile`: 500,000 bars, viewports of
   500, 2,000 and 8,000 buckets, five runs each, reporting individual and median
   times and the allocation count after the first call. Commit.
- [ ] 9. Run `make check`. Confirm `make wheel-test` shows an unchanged Python
   surface and `make parity-isolation` passes. Fix, re-run, commit.
- [ ] 10. **Human:** run `make qt-test` against a series loaded from a real lake
   dataset and `make bench-bar-geometry` on the target machine; report both.

## Validation

- **Unit:** every series ordering, capacity, forming and atomicity rule;
  revision and dirty-range rules; reduction count, boundary, exactness and NaN
  rules; packing coordinates, flags, determinism and degenerate viewports.
- **Integration:** the headless Rust drive of the Qt object; the C++ harness
  through `make qt-test`.
- **Regression:** every gate merged before this task; `RollingBarWindow` and its
  parity fixtures untouched; the wheel's surface unchanged;
  `make parity-isolation`.
- **Determinism:** double reduction and double packing in one process.
- **Measurement:** vertex production time and allocation count at three bucket
  counts over 500,000 bars.

```bash
cd /home/gui/projects/q/q_core
make check
cargo test -p q-buffers
cargo test -p q-qt
make qt-test

# human (step 10)
make bench-bar-geometry
```

## Handoff

Report the measured vertex production times at 500, 2,000 and 8,000 buckets and
the steady-state allocation count. Report the capacity-drop cost at 500,000
bars. Confirm `RollingBarWindow`, its tests and its fixtures are untouched, and
that the wheel exposes nothing new. State the `q_core` release tag this task
produces, since Q-035 and Q-037 pin it.
