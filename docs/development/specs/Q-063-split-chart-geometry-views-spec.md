# Q-063: Split completed and forming chart geometry views

**Status:** plan awaiting review; status of record is the [Q project board](https://github.com/users/GuilhermeFortuna/projects/2)
**Depends on:** Q-034
**Consumer:** [q_terminal Q-062](https://github.com/GuilhermeFortuna/q_terminal/issues/21)
**Implementation plan:** [`../plans/Q-063-split-chart-geometry-views-plan.md`](../plans/Q-063-split-chart-geometry-views-plan.md)

## Purpose

Q-062 has a measured live-candle benchmark but cannot keep completed scene
graph geometry resident while the pinned `q-qt::BarSeries` supplies only one
combined vertex view. A forming update advances the series revision and the
current `rebuild_geometry()` reduces and packs all visible completed bars,
appends the forming bucket, and copies all vertices into its CXX type. Give
the terminal independent completed and forming geometry views so a fixed-view
forming tick touches only the forming candle.

This task produces the Rust and CXX-Qt API and a released `q_core` tag. The
terminal's Qt node changes and dependency pin belong to Q-062.

## Current system

`crates/q-buffers/src/lod.rs::reduce_into()` already reuses a bucket vector and
derives exact deterministic bucket boundaries from the completed range and
surface width. `crates/q-buffers/src/geometry.rs::pack()` emits 12 vertices per
bucket, with surface x/y and direction/forming flags. The forming bar is held
apart from completed data in `LiveBarSeries`, but
`crates/q-qt/src/bar_series.rs::rebuild_geometry()` combines the two before
packing. Its `geom_vertices` and FFI `vertices` are separate reusable vectors;
the conversion between their distinct types is a safe copy.

The terminal currently calls `set_viewport()`, `set_surface()`, and
`rebuild_geometry()`, then reads `vertex_ptr()`, `vertex_len()`, and
`geometry_revision()` during scene graph synchronization. Those methods must
remain available while Q-062 waits for a released tag.

## Required interface and behavior

- Add a `rebuild_split_geometry()` method to `BarSeries` alongside the legacy
  combined method. Add `completed_vertex_ptr()`, `completed_vertex_len()`,
  `completed_geometry_revision()`, `forming_vertex_ptr()`,
  `forming_vertex_len()`, and `forming_geometry_revision()`. Pointers refer to
  the existing CXX `BarVertex` layout and are null for an empty view. They
  remain valid until a later mutable method call on that `BarSeries`; callers
  read both views during the existing synchronized render phase.
- `rebuild_split_geometry()` caches completed output against completed-data
  generation and exact viewport/surface inputs. Forming replacement alone
  does not advance the completed geometry revision, modify completed vertex
  bytes, or change its pointer. A visible forming update packs at most one
  12-vertex bucket. An offscreen forming update leaves both views unchanged.
- History replacement, completed append or replacement, capacity eviction,
  viewport/LOD change, price transform change, and surface-size change
  invalidate completed output. A new completed bar may move the forming index
  or change LOD boundaries. Forming presence, content, visibility, or shared
  transform changes invalidate the forming output; clearing it yields length
  zero and a revision change when prior visible geometry existed.
- Preserve `q-buffers::geometry::pack()` output, bucket ordering, flags, and
  coordinate arithmetic. Introduce a reusable single-bucket packing helper
  rather than duplicating geometry formulas. For a valid view, concatenating
  completed and forming split views must equal the legacy combined vertices
  byte for byte. Empty, zero-size, flat-price, narrow, and LOD views must
  retain their current behavior.
- Maintain reusable capacity so warmed-up forming replacements allocate no
  Rust geometry storage. The safe `q-buffers` vertex to CXX vertex copy may
  remain, but on a forming-only rebuild it is limited to the forming slice.
  Do not use unchecked casts between the distinct Rust and CXX types.
- Do not change market-data ingestion semantics, `LiveBarSeries` public bar
  contract, Python bindings, wire contracts, Qt rendering code, or LOD
  reduction rules. Keep `q-buffers` free of Qt dependencies.

## Acceptance criteria

1. `q-buffers` tests prove split packing and legacy packing have identical
   vertices across rising/falling candles, wick/body degeneracy, one bar,
   reduced LOD buckets, and the last visible forming bucket.
2. `q-qt` Rust and C++ harness tests prove that repeated forming updates at
   fixed view/bounds retain completed revision, pointer, and bytes while the
   forming view changes. After warm-up, this path allocates no Rust geometry
   storage. An offscreen update changes neither geometry view.
3. Tests cover completed append/replacement and capacity eviction; history
   replacement; forming removal and reappearance; pan/zoom, price bounds,
   surface dimensions, zero-size and empty views. Revision changes correspond
   to the geometry view that changed, including clearing a nonempty view.
4. The legacy combined API still passes its current tests and C++ harness.
   `make check` passes in `q_core`; a split-path run of
   `make bench-bar-geometry` reports 500, 2,000, and 8,000 visible buckets
   over 500,000 bars, with fixed-transform forming updates compared against
   the current combined path.
5. The change is reviewed and released through the repository's normal
   `./work finish` workflow, yielding a pushed date tag that Q-062 can pin.

## Risks and tradeoffs

- A revision derived only from the aggregate series revision would still
  invalidate completed output on every forming tick. Track completed-data
  changes separately and include all view and transform inputs in the cache
  key.
- Two simultaneously live pointer views need stable ownership. Keep each
  slice in independent storage and avoid mutating either after handing its
  pointer to the synchronized renderer.
- Retaining the legacy combined API temporarily costs code and memory, but
  allows the currently pinned terminal to keep building until Q-062 migrates.
