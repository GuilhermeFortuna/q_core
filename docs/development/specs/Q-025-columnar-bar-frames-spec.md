# Q-025: Columnar bar frames

**Status:** authoritative in the [Q project board](https://github.com/users/GuilhermeFortuna/projects/2)  
**Project direction:** [`q_contracts/docs/system-architecture.md` §3.3, §4.6, §5, §9 invariants 1, 2 and 5](https://github.com/GuilhermeFortuna/q_contracts/blob/f2273a88e52c5b9a8ad5ac9d7eb27f8069cf643d/docs/system-architecture.md#46-visualization-data-path)  
**Depends on:** Q-021  
**Implementation plan:** [`../plans/Q-025-columnar-bar-frames-plan.md`](../plans/Q-025-columnar-bar-frames-plan.md)

## Purpose

Every bar that `q_backend` evaluates today lives in a pandas `DataFrame`: the
candle engine walks one row object per bar, and the execution worker's
`StrategyEvaluator` keeps its completed-bar history as a frame that it
concatenates, deduplicates, sorts and trims on every ingest. `q_core` has no
in-memory representation of bars at all, so the batch-05 candle kernel, the tick
kernel's bar output and the evaluator swap have nothing to consume and nothing
to hand back. This task gives `q-buffers` a columnar bar frame (open times,
prices, volumes, and named indicator and signal columns in an Arrow-compatible
layout), filled from numpy arrays through the wheel without per-row objects, and
a bounded rolling window that reproduces the evaluator's window exactly. The
frame's shape is fixed now, before three kernels are written against it, and
the window is proven equal to today's Python before the evaluator depends on it.

## Requirements

### Bar columns

- A frame carries a bar open time and the four prices under the names and types
  of the contracted bars Arrow schema in `q_contracts` (`time`, `open`, `high`,
  `low`, `close`). It also carries the contracted `tick_volume`, `spread` and
  `real_volume` columns with their contracted types when the source has them.
- The three volume columns are individually optional. A frame without them is
  valid, because the evaluator's window and the backtest goldens carry no
  contracted volume column today. A frame never fills a missing volume column
  with zeros or placeholders.
- The frame's description of its contracted columns (name, type, nullability,
  timezone marker) is checked against the vendored contract schema. It is not
  checked against a copy written into `q_core`, so a contract change that
  renames or retypes a bar column fails the check.
- Contracted column names are reserved. A derived column cannot take a reserved
  name, so `close` always means the bar's close.

### Derived columns

- A frame carries any number of named derived columns of type 64-bit float,
  boolean, 8-bit signed integer or 64-bit signed integer. These cover every
  column the candle strategies and exit rules add today: indicator values,
  `buy_signal`/`sell_signal`-style booleans, direction codes, and integer bar
  counters.
- Derived column names are non-empty and unique within a frame. Columns keep the
  order in which they were added, so iterating a frame's columns gives the same
  order on every run and every platform.
- Missing values in float columns are NaN, consistent with the indicator kernels.
  Boolean and integer columns have no missing values. A source that has missing
  values in such a column is rejected rather than coerced.
- Every column in a frame has exactly as many values as the frame has bars.

### Timestamps

- Bar times are bar open times, stored as signed 64-bit microsecond counts: the
  unit of the contracted `timestamp[us]` type.
- The frame applies no timezone conversion. The stored count is the wall-clock
  value it was given. The frame records which clock that is, either the
  contract's naive Brasília wall-clock marker (the lake's convention) or UTC
  (the backtest goldens' convention). No other label is accepted. Converting
  either clock to a UTC instant is the caller's business, as in Q-017.
- Within a frame, bar times are strictly increasing. A frame is never built from
  unsorted or duplicate times. Ordering and deduplication belong to the rolling
  window.
- A time that cannot be represented exactly in microseconds, or a missing time,
  is rejected. It is never truncated or rounded.

### Memory layout

- Each column is one contiguous buffer of little-endian fixed-width values with
  no offset and no validity buffer. Booleans are bit-packed least-significant
  bit first, with unused trailing bits zero. This is Arrow's layout for these
  types, so a later Arrow reader or writer can use the buffers without
  transforming values.
- Two frames built from the same inputs have byte-identical column buffers.

### Construction from Python

- The wheel builds a frame from numpy arrays and returns frame columns as numpy
  arrays. No Python object is created per bar or per value in either direction.
- Construction copies each input array into the frame at most once. Reading a
  column copies it out at most once. No whole-frame copy happens per bar
  appended to a window. This is the bounded-copy allowance in architecture §4.6.
- The wheel accepts float64, bool, int8 and int64 arrays for the matching column
  types, and datetime64 arrays in microseconds or nanoseconds for time. An array
  of any other dtype is rejected with an error that names the column and the
  dtype. It is never cast silently.
- Invalid input raises a Python exception and never aborts the interpreter.

### Rolling window

- A rolling window holds at most a fixed number of completed bars. The bound is
  set when the window is created and must be at least one.
- Ingesting a batch of completed bars produces the same window as
  `StrategyEvaluator.ingest_completed_bars` in `q_backend` does to its rolling
  frame. The batch is merged with the current window, and for each open time the
  bar delivered last wins: a later batch over an earlier one, and a later
  position over an earlier one within a batch. The result is ordered by open
  time, and only the newest bars up to the bound are kept.
- A batch may be unsorted, may overlap the window, may re-deliver bars with
  corrected values, and may contain bars older than anything in the window. An
  empty batch leaves the window unchanged.
- The window holds bar columns only, never derived columns. Indicators are
  recomputed over the window's current contents, as the evaluator does today,
  so no value in a window depends on a bar that has been trimmed away.
- Which volume columns a window carries is fixed at creation. A batch that does
  not match that column set is rejected, instead of being filled with NaN the
  way a pandas concatenation would.
- A window also holds at most one forming bar, separate from its completed bars.
  Setting a forming bar replaces any previous forming bar. The forming bar is
  never counted toward the bound and never appears among the completed bars.
  Ingesting a completed bar whose open time is at or after the forming bar's
  open time clears the forming bar. A forming bar whose open time is not after
  the newest completed bar is rejected. These rules match how
  `streaming/market/cursor.py` in `q_backend` promotes forming bars to
  completed.
- A window's completed bars can be taken as a frame, and its forming bar, if
  any, as a one-bar frame. Evaluation and indicators take the completed view.
- The per-ingest cost is proportional to the window bound and the batch size,
  not to how many bars have been ingested over the window's lifetime.

### Parity and determinism

- The window's parity with today's evaluator is proven by reference fixtures
  exported from `q_backend` through the Q-021 exporter and checked by the Q-021
  gate library. They are stored in Q-021's fixture format, and no second value
  encoding is introduced. The comparison is exact: times as integers and
  prices bit for bit. The window copies values and never computes them, so
  any tolerance would hide a defect.
- The window fixtures are exported at the same `q_backend` pin as the
  indicator fixtures. That pin predates the backend calling `q_core` (Q-023),
  so the reference is never `q_core` compared with itself.
- Reproducing the window fixtures needs the full `q_backend` environment,
  because the reference is the real evaluator. Whether CI runs that
  reproduction is decided explicitly and documented with its reason. A pin
  change without regenerated fixtures still fails the standard validation
  suite without that environment.
- The same sequence of window operations produces byte-identical frames when run
  twice in one process. This is checked by the Q-021 double-run determinism
  check.
- Forming-bar behaviour has no evaluator reference and is covered by unit tests
  that state each rule above as a case.

### Preserved guarantees

- `q-buffers` keeps its existing public items and its vendored contract modules.
  The dependency direction between crates is unchanged. `q-buffers` stays free
  of unsafe code, input and output, clocks and environment access.
- `q_core.version()` and `q_core.contracts_rev()` keep their names and return
  values.
- The wheel still builds without Qt, and `q-qt` still builds without Python.

## Constraints and non-goals

- **No Arrow or Parquet file or IPC reading or writing, and no Arrow
  record-batch export.** The layout is Arrow-compatible so that `q-io` can adopt
  it in Phase 3, when `q_terminal` gives it a real reader. Exporting a record
  batch now would pull in the Arrow library before anything consumes it.
- **No kernels.** No indicator is computed into a frame, no signal is evaluated,
  and no candle or tick loop consumes a frame. Q-022 owns the indicators, and
  Q-026, Q-027 and Q-029 own the loops.
- **No bar aggregation from ticks.** Building bars from ticks is a tick-kernel
  concern (Q-029), not a buffer concern.
- **No timeframe knowledge.** The frame stores open times only. It derives no
  close times and knows no bar durations. Close-time watermarks,
  `last_evaluated_close`, `through_close` and duplicate-decision detection stay
  in the Python evaluator until Q-031.
- **No change to `q_backend`.** The evaluator keeps its pandas window, the engine
  keeps its row loop, and the exporter reads `q_backend` at a pinned commit
  without modifying it. Q-028 and Q-031 swap the frame in.
- **No validity bitmaps or nullable columns.** NaN already expresses a missing
  float, and no bar or signal column the kernels will read is nullable in a way
  NaN cannot express. All-null lake volume columns (`q_contracts` catalog
  Finding 6) are absent columns here, not null ones.
- **No zero-copy numpy views over frame memory.** A view into a window that the
  next ingest rewrites would expose memory that has changed under it. Copies
  bounded by the window size are within §4.6.
- **No string, categorical or timestamp-typed derived columns.** No strategy
  column the batch-05 kernels read has those types. Adding them without a
  consumer would be a guess.
- **No Qt projection of frames.** `q_terminal`'s chart buffer is Phase 3. `q-qt`
  only has to keep building.
- **No move of the `q_backend` reference pin.** The window fixtures use the pin
  Q-021 established. Moving it is a separate, deliberate change that
  regenerates every family, and it can never go past Q-023.
- **No performance gate.** Ingest cost is measured and reported against the
  pandas window, but no threshold fails the build. The evaluator benchmark in
  `q_backend` stays the gate, and it moves when Q-031 swaps the window in.

## Acceptance criteria

### Agent-verifiable

1. A frame built from bar columns reports contracted column names, types,
   nullability and timezone marker equal to the vendored `q_contracts` bars
   Arrow schema. Changing the vendored schema's `close` type in a scratch copy
   makes that test fail, and the change is reverted.
2. Building a frame rejects: a derived column with a reserved name, a duplicate
   derived name, a column whose length differs from the time column, a
   non-increasing time sequence, and a timezone label other than the two
   accepted ones. Each case is a unit test.
3. Float, int8, int64 and boolean columns expose buffers of the Arrow layout.
   A 10-value boolean column is 2 bytes with bits 10 to 15 zero, and a float
   column's bytes are the little-endian encoding of its values. This is unit
   tested.
4. Through the installed wheel, a frame built from numpy arrays returns every
   column as a numpy array of the same dtype, with values equal bit for bit.
   Nanosecond times that are exact multiples of 1,000 are accepted.
   Nanosecond times with a sub-microsecond remainder, `NaT`, float16 prices, and
   an object-dtype boolean column are each rejected with a Python exception that
   names the column.
5. Rolling-window reference fixtures exported from `q_backend`'s
   `StrategyEvaluator` pass the Q-021 gate under the exact policy. They use the
   same backend pin as the indicator fixtures, which predates Q-023, and carry
   Q-021 provenance. They cover:
   - append;
   - re-delivery with changed values;
   - overlapping batches;
   - a late bar older than a full window;
   - a late bar filling a gap in a partial window;
   - an unsorted batch with duplicate open times;
   - an empty batch;
   - a bound of 1.

   Negative controls each fail:
   - one expected price moved by one ULP;
   - one expected time moved by one microsecond;
   - a corrupted value that no longer matches its column checksum;
   - an unaccounted extra fixture file.
6. The Q-021 double-run determinism check passes for every window scenario,
   and fails for a replay that changes its output on its second call.
7. Forming-bar unit tests pass: replacement, exclusion from the bound and from
   completed bars, clearing on promotion, clearing when a newer completed bar
   arrives, and rejection of a forming bar that is not newer than the newest
   completed bar.
8. A batch whose volume columns differ from the window's, and a window bound of
   zero, are rejected.
9. The vendored bars schema is covered by `make contracts-check`. A clean
   regeneration and re-vendoring produce no diff.
10. `q_core.version()` and `q_core.contracts_rev()` still pass the existing
    wheel test, and `q-qt` still builds and passes its harness.
11. The full validation suite passes: `make check`.

### Human-verifiable

1. Rolling-window ingest cost is compared with the pandas window update it
   replaces: one-bar ingests into a full 605-bar window (the bound
   `compute_window_bound_bars` gives a 200-period lookback), over five runs of
   10,000 ingests each. Individual and median per-ingest times are reported for
   both.
   Command: `cd q_core && make bench-bar-window`
2. The frame's column model is reviewed against the columns the candle
   strategies, the genome `CompositeStrategy` and the exit rules add in
   `q_backend`. The review confirms that each one maps to a supported derived
   column type, and it lists any that do not.
   Command: `cd q_core && cargo doc -p q-buffers --no-deps --open`
3. The window fixtures are regenerated from the pinned backend in its full
   environment and show no diff. The time and disk space the regeneration
   needs are reported, and they back the recorded decision on whether CI runs
   this check.
   Command: `cd q_core && time make fixtures-backend-check`
