# Q-034: Live bar series projected to Qt

**Status:** authoritative in the [Q project board](https://github.com/users/GuilhermeFortuna/projects/2)  
**Project direction:** [`q_contracts/docs/system-architecture.md` §4.6, §5, §5.1, §10 Phase 3](https://github.com/GuilhermeFortuna/q_contracts/blob/a9724767015905f1e9cd5d98ba492080910db4f2/docs/system-architecture.md#46-visualization-data-path)  
**Depends on:** Q-033  
**Implementation plan:** [`../plans/Q-034-live-bar-series-projected-to-qt-plan.md`](../plans/Q-034-live-bar-series-projected-to-qt-plan.md)

## Purpose

The slice renders one symbol's bars from a `q_core` buffer, and architecture
§4.6 puts downsampling, binning and level-of-detail in `q_core` on the CPU,
never on a UI thread, with stream updates coalesced to one upload per frame.
`q_core` has a rolling window sized for strategy evaluation (Q-025) and now has
codecs (Q-033), but nothing that holds a growing display series, merges a
forming bar into it, decides what a pixel column shows, or hands the result
across the Qt boundary. `q-qt` exposes only a version string. This task adds the
live series, its level-of-detail reduction, and the Qt projection that turns
both into vertex data a scene-graph node can upload without computing anything.

## Requirements

### The series

- A series holds one symbol and timeframe's completed bars in time order, up to a
  capacity fixed when it is created. Appending past capacity drops the oldest
  bars and nothing else.
- Appending a batch whose first bar is later than the current last bar extends
  the series. A bar whose time equals the last bar's replaces it. A batch that
  would land before the last bar, or that is not itself ordered, is refused with
  an error saying so, leaving the series unchanged.
- A forming bar is held apart from the completed bars and can be replaced at any
  rate. Replacing it never touches the completed bars. A completed bar at the
  forming bar's time clears the forming bar.
- Loading history replaces the whole series atomically: either the new bars are
  in place or the previous ones are, never a mixture.
- A series reports its length, its time and price extents, its last price, and
  whether a forming bar is present.

### Change tracking

- Every accepted mutation advances a revision counter that never repeats or goes
  backwards within a series' life.
- Alongside the revision, the series reports the range of bars that changed since
  a given earlier revision, so a consumer can decide between a partial and a full
  refresh. A revision too old to describe is reported as such, and the consumer's
  correct response is a full refresh.

### Level of detail

- Given a half-open bar range and a number of pixel columns, the series reduces
  the range to at most that many buckets. Each bucket carries the open of its
  first bar, the close of its last, the highest high, the lowest low, and the
  bar range it covers.
- The reduction is deterministic: the same range, the same column count and the
  same bars always give the same buckets, with bucket boundaries derived only
  from the range and the column count.
- A range no longer than the column count is not reduced; each bar is its own
  bucket.
- Reduction is exact: a bucket's extremes are the extremes of its bars, never
  sampled or interpolated.
- An empty range gives no buckets. A zero column count is an error.

### Qt projection

- A Qt object exposes one series to QML: its symbol, timeframe, bar count,
  revision, last price, whether a forming bar is present, and the current time
  and price extents, each notifying on change.
- QML can set the viewport — the bar range and the price range to display — and
  the pixel size of the surface that will draw it. Setting them is cheap and does
  not itself decode, allocate per bar, or block.
- From the viewport, the object produces packed vertex data in surface
  coordinates: for each bucket, the geometry of its body and its wick, with a
  flag saying whether the bucket closed above or below its open. The forming
  bucket, when present, is the last one and is flagged as forming.
- C++ obtains that data as a pointer and a length valid for the duration of one
  render pass, together with the revision and viewport it was built from, so the
  renderer can tell whether it already holds it. No copy of the series crosses
  the boundary.
- Producing vertex data twice from the same series, viewport and surface size
  gives byte-identical bytes.
- Ingesting a batch, replacing the forming bar, and loading history are callable
  from Rust without a Qt event loop, so the data path is testable headless.

### Cost

- Producing vertex data for 2,000 visible buckets from a series of 500,000 bars
  stays under 2 ms, measured on the target machine, and allocates nothing after
  the first call for a given surface size.
- Replacing the forming bar is constant-time and allocation-free.

### Boundaries preserved

- The series, its reduction and its vertex packing are ordinary Rust in a crate
  that knows nothing of Qt; the Qt crate holds the object, the properties and
  the pointer handoff, and no arithmetic over bars.
- `q_core` still knows nothing of networks, catalogs, symbols' provenance, or
  where bars came from. It is handed frames.
- Every gate merged before this task keeps passing, the wheel still builds
  without Qt, and `q-parity` stays out of the `q-py` and `q-qt` dependency trees.

## Constraints and non-goals

- **No rendering.** This task produces bytes to upload. The scene-graph node,
  the shaders and the frame loop are Q-037's, in `q_terminal`.
- **No networking, no REST, no WebSocket.** Frames arrive from a caller.
- **No axes, grids, crosshair, labels, or time-axis formatting.** Those are
  presentation and belong to the terminal.
- **No indicators or overlays on the series.** The slice draws bars.
- **No multi-series or multi-pane state.** One series per object; a second pane
  is a second object.
- **No Python exposure.** The wheel's surface is unchanged.
- **No change to `RollingBarWindow`.** The evaluation window and the display
  series have different rules — the window deduplicates and re-sorts a batch,
  the display series refuses one — and unifying them would change evaluation.

## Acceptance criteria

### Agent-verifiable

1. Unit tests state the series rules as cases: append extends; an equal-time bar
   replaces the last; an out-of-order batch and an unordered batch are both
   refused and leave the series unchanged; capacity drops the oldest; a forming
   bar replaced a thousand times leaves the completed bars untouched; a completed
   bar at the forming time clears it; a failed history load leaves the previous
   series intact.
2. Unit tests state revision and dirty-range rules: every accepted mutation
   advances the counter; a refused one does not; the reported range covers
   exactly the changed bars; a revision older than the tracked horizon reports
   as too old.
3. Unit tests state the reduction rules: bucket count never exceeds the column
   count; boundaries depend only on range and columns; extremes are exact; a
   short range is unreduced; an empty range gives nothing; zero columns is an
   error.
4. Unit tests state the packing rules: body and wick geometry for a known bar at
   a known viewport and surface size; the up/down flag; the forming flag on the
   last bucket; identical bytes on a second call; a price range of zero height
   and a bar range of zero width both produce well-formed, non-NaN output.
5. A headless test drives the Qt object through history load, completed ingest
   and forming replacement without an event loop, and asserts the exposed
   properties and the pointer handoff's revision and length.
6. A benchmark reports vertex production time for 2,000 buckets over 500,000
   bars and the allocation count on the second and later calls.
7. Existing gates pass unchanged, including `make qt-test`, `make wheel-test`
   and `make parity-isolation`.
8. The full validation suite passes: `make check`.

### Human-verifiable

1. The Qt harness is run against a series loaded from a real lake dataset, and
   the reported extents, bar count and last price are checked against the same
   dataset read by the backend.
   Command: `cd q_core && make qt-test`
2. Vertex production timing is reported from the target machine, five runs,
   individual and median, for 500, 2,000 and 8,000 buckets.
   Command: `cd q_core && make bench-bar-geometry`
