# Q-063 implementation plan: Split completed and forming chart geometry views

> **For implementation agents:** Read the linked spec, `README.md`, and
> `RELEASING.md`. Work only on the task branch created by
> `./work start Q-063 --agent <agent> --worktree`. Track these steps with their
> checkboxes; keep the existing combined API working until q_terminal Q-062
> consumes a released tag.

**Goal:** Expose independently cached completed and forming candle vertices
through `q-qt`, with forming ticks leaving completed geometry untouched.
**Architecture:** `q-buffers` retains one authoritative packing formula;
`q-qt::BarSeries` owns separate reusable completed/forming buffers and
invalidation keys. CXX-Qt exposes two pointer/length/revision views.
**Tech stack:** Rust, `q-buffers`, CXX-Qt, C++ harness.
**Spec:** [`../specs/Q-063-split-chart-geometry-views-spec.md`](../specs/Q-063-split-chart-geometry-views-spec.md)

## File map and interface

- `crates/q-buffers/src/geometry.rs`: factor the existing per-bucket packing
  formula into one shared helper; keep `pack()` unchanged for its callers.
- `crates/q-qt/src/bar_series.rs`: add split buffers, separate invalidation
  state, and the CXX-Qt methods named in the spec. Keep `rebuild_geometry()`
  and the current combined accessors unchanged for old consumers.
- `crates/q-qt/tests/harness.cpp`: verify the new methods through generated
  C++ bindings as well as through Rust unit tests in `bar_series.rs`.
- `Makefile` or existing bar-geometry benchmark source: add a split forming
  workload to `make bench-bar-geometry`, retaining its existing workload.

The new call sequence is `set_viewport` / `set_surface` →
`rebuild_split_geometry` → read both `*_vertex_ptr`, `*_vertex_len`, and
`*_geometry_revision` values. A null pointer accompanies an empty slice.
Pointers remain valid until the next mutable method call; the terminal reads
them in Qt's synchronized scene graph phase. Each geometry revision advances
when its corresponding slice is regenerated or cleared from a nonempty state,
and stays fixed on unrelated ticks. `q-qt` tracks completed-data generation
independently of the aggregate series revision, which also advances on forming
replacement.

## Ordered implementation

- [x] **1. Lock output parity in `q-buffers`.** Add tests in
  `crates/q-buffers/src/geometry.rs` that pack completed buckets and a final
  forming bucket separately, concatenate the outputs, and compare all four
  vertex fields bit for bit with one legacy `pack()` call. Cover rising and
  falling bodies, flat prices, a one-bar view, and LOD buckets from
  `reduce_into()`. Factor a single-bucket helper from `pack()` so both paths
  use the same x/y and flag arithmetic. Run the crate's focused tests.

- [x] **2. Add split storage and invalidation to `q-qt`.** In
  `crates/q-qt/src/bar_series.rs`, add independent reusable `q-buffers` and
  FFI vertex vectors for completed and forming output, plus their revisions.
  Increment completed-data generation after successful history or completed
  mutations, including replacements and eviction; never on forming-only
  mutations. Build a completed cache key from that generation and exact view
  range, price bounds, and surface dimensions. Build the forming cache key from
  forming data/presence and the same transform. `rebuild_split_geometry()`
  reduces and packs completed bars only on completed-key change, and packs the
  visible forming bucket alone only on forming-key change. Clear stale output
  for empty, invalid, zero-size, and offscreen states. Keep legacy storage and
  `rebuild_geometry()` operational. Add focused Rust tests for each invalidation
  and for unchanged completed pointer, bytes, and revision across forming ticks.

- [x] **3. Expose and verify the CXX-Qt API.** Declare and implement
  `rebuild_split_geometry()` and the six pointer/length/revision accessors in
  `crates/q-qt/src/bar_series.rs`. Extend
  `crates/q-qt/tests/harness.cpp` to check both slices' lengths, flag values,
  pointer validity, stable completed pointer/revision on a forming tick,
  forming clear, and full invalidation on completed append and viewport
  change. Run `make qt-test` and the existing combined-API harness cases.

- [x] **4. Measure and finish.** Extend `make bench-bar-geometry` with a
  500,000-bar split-path forming workload at 500, 2,000, and 8,000 visible
  buckets. Warm it before reporting time and allocations; compare the existing
  combined path and the split path, and verify that the completed revision and
  pointer stay fixed during forming-only iterations. Run `make check` in
  `q_core`, record results in the task handoff, commit focused changes, and
  move Q-063 to In Review with `./work board set`. The human's
  `./work finish Q-063` performs the reviewed release and pushes its tag;
  send the tag and commit to the Q-062 implementer for the terminal pin update.

## Review focus

- A forming bar leaving the viewport clears a previously visible forming
  slice without changing completed geometry.
- Completion and capacity eviction cannot reuse stale LOD buckets.
- Two pointers remain valid together throughout the synchronized read.
- The legacy combined API and Python wheel continue to pass their gates.
