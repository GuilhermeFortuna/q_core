# Q-025 implementation plan: Columnar bar frames

**Status:** authoritative in the [Q project board](https://github.com/users/GuilhermeFortuna/projects/2)  
**Specification:** [`../specs/Q-025-columnar-bar-frames-spec.md`](../specs/Q-025-columnar-bar-frames-spec.md)  
**Depends on:** Q-021

## Current-system context

`crates/q-buffers/src/lib.rs` (37 lines) is a skeleton. It is
`#![forbid(unsafe_code)]`, declares `CRATE_NAME`, and includes the vendored
`contracts/api.rs`, `catalog.rs` and `stream.rs` as `pub mod contracts` through
`#[path = "../../../contracts"]`. Its only dependencies are `serde` and
`serde_json`. `q-py` (`crates/q-py/src/lib.rs`, 21 lines) uses `pyo3 0.24.2`
with `abi3-py312`, exposes `version()` and `contracts_rev()`, and has no `numpy`
crate. The workspace lint table denies `float_cmp`, `float_cmp_const`,
`imprecise_flops` and `suboptimal_flops`, and `clippy.toml` disallows
`Instant`, `SystemTime` and `env::var`/`var_os`. `make contracts` copies only
`generated/rust/`, and `make contracts-check` runs `diff -ru` of that tree
against a regeneration at `CONTRACTS_REV` (`998a505`). `tests/test_wheel.py`
installs the wheel into a bare `uv venv` with no numpy. At that pin,
`q_contracts/schema/api/arrow/bars.schema.json` declares `time timestamp[us]`
with `tz: "naive-wallclock-America/Sao_Paulo"`, `open`/`high`/`low`/`close
float64`, and `tick_volume`/`spread`/`real_volume int64`, all non-nullable. The
file is byte-identical at `q_contracts` HEAD. It is also the declared payload of
the `bars.forming` and `bars.completed` topics in `schema/stream/topics.yaml`.
No generated Rust type describes it.

In `q_backend`, `StrategyEvaluator.ingest_completed_bars`
(`execution/evaluator.py:186-217`) sorts the batch, concatenates it after
`self._rolling`, keeps `~index.duplicated(keep="last")`, sorts again, and calls
`trim_rolling_window` (`execution/bars.py:63`: `iloc[-max_bars:]`, no trim when
`max_bars <= 0`). An empty batch returns before the merge (line 192).
`seed_window` (line 168) sorts and trims but does not deduplicate. The bound is
`compute_window_bound_bars` (`execution/warmup.py:59`), which is
`longest * _WARMUP_MULTIPLIER (3) + _WINDOW_BUFFER_BARS (5)`. The window holds
bars only, and `augment_indicator_frame` recomputes indicators over all of it
for each evaluated bar. The window has no forming bar: `drop_forming_bar`
(`bars.py:48`) removes it upstream in `bar_coordinator.py:130`. Forming-to-
completed promotion lives in `streaming/market/cursor.py` `bar_transitions`.
Column names are inconsistent. `ohlcv_list_to_frame` keeps
`("open", "high", "low", "close", "volume")`, but the `OHLCV` model has
`tick_volume`/`spread`/`real_volume` and no `volume`, so the live window
carries only OHLC. `_execute_candle` (`api/backtest_jobs.py:155`) keeps the
contracted names, and the goldens' `synthetic_ohlcv` uses a float `volume`.
Strategies add float indicators (`donchian_upper`, `macd_signal`, `rsi`,
`momentum`, `volatility`), boolean signals (`buy_signal`, `sell_signal`,
`exit_signal`, `exit_long_signal`, `exit_short_signal`, `rebalance`,
`entry_long_signal`), and int64 `bar_index` (`lai_lau_common.py:14`).
The gap is that `q_core` has no bar representation for the kernels to take,
and nothing proves that a Rust window equals this pandas one.

## Interfaces produced

```rust
// crates/q-buffers/src/column.rs
/// Physical type of a column, named with the q_contracts Arrow type strings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColumnType { TimestampMicros, Float64, Int64, Int8, Bool }
impl ColumnType {
    pub fn arrow_type(self) -> &'static str; // "timestamp[us]" | "float64" | "int64" | "int8" | "bool"
}

/// Bit-packed booleans, LSB first, trailing bits zero (Arrow boolean layout).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bitmap { /* bytes: Vec<u8>, len: usize */ }
impl Bitmap {
    pub fn from_bools(values: &[bool]) -> Self;
    pub fn len(&self) -> usize;
    pub fn is_empty(&self) -> bool;
    pub fn get(&self, index: usize) -> bool;
    pub fn as_bytes(&self) -> &[u8];          // ceil(len / 8) bytes
    pub fn to_bools(&self) -> Vec<bool>;
}

/// One owned column buffer. Float NaN = missing; other types have no missing values.
#[derive(Clone, Debug)]
pub enum Column { Float64(Vec<f64>), Int64(Vec<i64>), Int8(Vec<i8>), Bool(Bitmap) }
impl Column {
    pub fn len(&self) -> usize;
    pub fn column_type(&self) -> ColumnType;
    pub fn bytes_eq(&self, other: &Column) -> bool; // bitwise, incl. NaN payloads; the lint-safe equality
}
```

```rust
// crates/q-buffers/src/frame.rs
/// Which wall clock the stored microsecond counts are. No conversion is ever applied.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimeLabel { BrasiliaWallclock, Utc }
impl TimeLabel {
    pub fn contract_marker(self) -> &'static str; // "naive-wallclock-America/Sao_Paulo" | "UTC"
    pub fn parse(marker: &str) -> Result<Self, FrameError>;
}

/// Contracted bar columns, in any order; validated into a BarFrame or merged by a window.
#[derive(Clone, Debug)]
pub struct BarColumns {
    pub time: Vec<i64>,               // bar OPEN time, microseconds on `label`'s wall clock
    pub open: Vec<f64>,
    pub high: Vec<f64>,
    pub low: Vec<f64>,
    pub close: Vec<f64>,
    pub tick_volume: Option<Vec<i64>>, // contract: ticks
    pub spread: Option<Vec<i64>>,      // contract: points at bar close
    pub real_volume: Option<Vec<i64>>, // contract: contracts
    pub label: TimeLabel,
}

/// Which optional contracted volume columns are present.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VolumeSet { pub tick_volume: bool, pub spread: bool, pub real_volume: bool }

/// One field description, shaped like a q_contracts Arrow schema field.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FieldDesc { pub name: String, pub arrow_type: &'static str, pub nullable: bool, pub tz: Option<&'static str> }

pub const RESERVED_COLUMNS: [&str; 8] =
    ["time", "open", "high", "low", "close", "tick_volume", "spread", "real_volume"];

/// Immutable-shape columnar bar frame: strictly increasing open times, bar columns,
/// then derived columns in insertion order.
#[derive(Clone, Debug)]
pub struct BarFrame { /* bars: BarColumns, derived: Vec<(String, Column)> */ }
impl BarFrame {
    pub fn try_new(bars: BarColumns) -> Result<Self, FrameError>;
    pub fn len(&self) -> usize;
    pub fn is_empty(&self) -> bool;
    pub fn label(&self) -> TimeLabel;
    pub fn volume_set(&self) -> VolumeSet;
    pub fn time(&self) -> &[i64];
    pub fn open(&self) -> &[f64];
    pub fn high(&self) -> &[f64];
    pub fn low(&self) -> &[f64];
    pub fn close(&self) -> &[f64];
    pub fn tick_volume(&self) -> Option<&[i64]>;
    pub fn spread(&self) -> Option<&[i64]>;
    pub fn real_volume(&self) -> Option<&[i64]>;
    pub fn bars(&self) -> &BarColumns;
    pub fn push_column(&mut self, name: &str, column: Column) -> Result<(), FrameError>;
    pub fn column(&self, name: &str) -> Option<&Column>;
    pub fn column_names(&self) -> Vec<&str>;          // bar columns present, then derived, in order
    pub fn schema(&self) -> Vec<FieldDesc>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FrameError {
    LengthMismatch { column: String, expected: usize, actual: usize },
    NonIncreasingTime { index: usize },
    ReservedName(String),
    DuplicateName(String),
    EmptyName,
    UnknownTimeLabel(String),
    VolumeSetMismatch { expected: VolumeSet, actual: VolumeSet },
    LabelMismatch { expected: TimeLabel, actual: TimeLabel },
    ZeroBound,
    StaleForming { forming_time: i64, newest_completed: i64 },
    FormingNotSingleBar { rows: usize },
}
impl std::fmt::Display for FrameError { /* names the column or index */ }
impl std::error::Error for FrameError {}
```

```rust
// crates/q-buffers/src/window.rs
/// Bounded completed-bar window with at most one forming bar, equal to
/// StrategyEvaluator's pandas window for completed bars.
#[derive(Clone, Debug)]
pub struct RollingBarWindow { /* bound: usize, completed: BarFrame, forming: Option<BarFrame> */ }
impl RollingBarWindow {
    pub fn new(bound: usize, volumes: VolumeSet, label: TimeLabel) -> Result<Self, FrameError>;
    pub fn bound(&self) -> usize;
    pub fn len(&self) -> usize;                         // completed bars only
    pub fn is_empty(&self) -> bool;
    pub fn ingest_completed(&mut self, batch: BarColumns) -> Result<(), FrameError>;
    pub fn set_forming(&mut self, bar: BarColumns) -> Result<(), FrameError>; // exactly one row
    pub fn clear_forming(&mut self);
    pub fn completed(&self) -> &BarFrame;
    pub fn forming(&self) -> Option<&BarFrame>;
}
```

```rust
// crates/q-buffers/src/lib.rs   (additions; existing items unchanged)
pub mod column;
pub mod frame;
pub mod window;
pub use column::{Bitmap, Column, ColumnType};
pub use frame::{BarColumns, BarFrame, FieldDesc, FrameError, TimeLabel, VolumeSet, RESERVED_COLUMNS};
pub use window::RollingBarWindow;
```

```rust
// crates/q-py/src/frame.rs   — projection only
#[pyclass(name = "BarFrame", module = "q_core")]
pub struct PyBarFrame { /* inner: q_buffers::BarFrame */ }
#[pymethods]
impl PyBarFrame {
    #[staticmethod]
    #[pyo3(signature = (time, open, high, low, close, *, tick_volume=None, spread=None, real_volume=None, tz="naive-wallclock-America/Sao_Paulo"))]
    fn from_numpy(py: Python<'_>, time: &Bound<'_, PyAny>, open: PyReadonlyArray1<'_, f64>, high: PyReadonlyArray1<'_, f64>,
                  low: PyReadonlyArray1<'_, f64>, close: PyReadonlyArray1<'_, f64>, tick_volume: Option<PyReadonlyArray1<'_, i64>>,
                  spread: Option<PyReadonlyArray1<'_, i64>>, real_volume: Option<PyReadonlyArray1<'_, i64>>, tz: &str) -> PyResult<Self>;
    fn add_column(&mut self, name: &str, values: &Bound<'_, PyAny>) -> PyResult<()>; // float64 | bool | int8 | int64 ndarray
    fn column<'py>(&self, py: Python<'py>, name: &str) -> PyResult<Bound<'py, PyAny>>; // copy; datetime64[us] for "time"
    fn column_names(&self) -> Vec<String>;
    fn schema(&self) -> Vec<(String, String, bool, Option<String>)>;
    #[getter] fn tz(&self) -> &'static str;
    fn __len__(&self) -> usize;
}

#[pyclass(name = "RollingBarWindow", module = "q_core")]
pub struct PyRollingBarWindow { /* inner: q_buffers::RollingBarWindow */ }
#[pymethods]
impl PyRollingBarWindow {
    #[new]
    #[pyo3(signature = (bound, *, tick_volume=false, spread=false, real_volume=false, tz="naive-wallclock-America/Sao_Paulo"))]
    fn new(bound: usize, tick_volume: bool, spread: bool, real_volume: bool, tz: &str) -> PyResult<Self>;
    #[pyo3(signature = (time, open, high, low, close, *, tick_volume=None, spread=None, real_volume=None))]
    fn ingest_completed(&mut self, /* same array parameters as from_numpy, minus tz */) -> PyResult<()>;
    #[pyo3(signature = (time, open, high, low, close, *, tick_volume=None, spread=None, real_volume=None))]
    fn set_forming(&mut self, /* same, one row each */) -> PyResult<()>;
    fn clear_forming(&mut self);
    fn completed(&self) -> PyBarFrame;          // bounded copy of <= bound rows
    fn forming(&self) -> Option<PyBarFrame>;
    #[getter] fn bound(&self) -> usize;
    fn __len__(&self) -> usize;
}

// crates/q-py/src/lib.rs: q_core module adds the two classes; version()/contracts_rev() unchanged.
// crates/q-py/Cargo.toml: numpy = "0.24" (the release paired with pyo3 0.24)
// pyproject.toml: [project] dependencies = ["numpy>=2"]
```

```
contracts/schema/api/arrow/bars.schema.json   vendored byte copy at CONTRACTS_REV (make contracts)
Makefile                                       contracts / contracts-check vendor and diff the schema file;
                                               wheel-test also runs tests/test_bar_frame.py; new bench-bar-window target
tests/test_bar_frame.py                        wheel-level numpy round-trip and rejection tests (run in the wheel venv)
tests/bench_bar_window.py                      human measurement script (not part of make check)
fixtures/reference/bar_window/<scenario_id>.json   one fixture per scenario, Q-021 envelope, environment "backend"
tools/reference/families/bar_window.py         BarWindowFamily, registered in FAMILIES in tools/reference/export_reference.py
tools/reference/test_export_reference.py       bar_window encoding/stub tests added (stdlib unittest, run by fixtures-test)
Makefile                                       BACKEND_FAMILIES := bar_window (NUMERIC_FAMILIES unchanged)
crates/q-buffers/Cargo.toml                    [dev-dependencies] q-parity
crates/q-buffers/tests/bar_window_gate.rs      the bar_window gate
README.md                                      fixture protocol: bar_window is a backend family; fixtures-backend-check
                                               is required locally before merging changes to it or to BACKEND_REV
```

```jsonc
// fixtures/reference/bar_window/<scenario_id>.json  (family "bar_window": operation sequence -> window after each step)
{ "format": "q-core-reference-fixture/1", "family": "bar_window", "fixture_id": "s07_unsorted_duplicates",
  "policy": { "kind": "exact" },
  "provenance": { "backend_repo", "backend_rev", "exporter", "environment": "backend", "python", "numpy", "pandas",
                  "sources": [ { "path": "src/q_backend/execution/evaluator.py", "blob" },
                               { "path": "src/q_backend/execution/bars.py", "blob" },
                               { "path": "tests/backtesting/test_goldens.py", "blob" } ] },
  "cases": [ { "case_id": "s07_unsorted_duplicates/step=0",
               "params": { "bound": 20, "tz": "UTC" },                // identical on every step of a file
               "operation": "seed_window" | "ingest_completed_bars",  // the evaluator method the exporter called
               "batch":  { "time": { "dtype": "int64", ... }, "open": { "dtype": "float64", ... }, "high", "low", "close" },
               "expected": { "window": { "time", "open", "high", "low", "close" } } } ] }
// every column is a Q-021 column object: dtype, bits_fnv1a64, values; time is int64 microseconds of bar open time
```

```python
# tools/reference/families/bar_window.py   (environment "backend": imports q_backend normally)
SCENARIO_IDS: Final = ("s01_seed", "s02_append", "s03_redeliver", "s04_overlap", "s05_late_full",
                       "s06_gap_fill", "s07_unsorted_duplicates", "s08_empty", "s09_bound_one")

class _StubStrategy:   # compute_indicators returns its input; check_entry_conditions / check_exit_conditions return []
    ...

class BarWindowFamily:   # name "bar_window", environment "backend", policy {"kind": "exact"}
    def export(self, source: BackendSource, out_dir: Path) -> list[str]: ...
def scenario_steps(bars: pd.DataFrame) -> dict[str, tuple[int, list[tuple[str, pd.DataFrame]]]]: ...  # id -> (bound, steps)
def encode_time(index: pd.DatetimeIndex) -> dict[str, object]: ...   # tz_convert(None), exact ns -> us, int64 column
```

```rust
// crates/q-buffers/tests/bar_window_gate.rs
const BACKEND_REV: &str = include_str!("../../../BACKEND_REV");
const BOUND_SCENARIOS: &[&str] = &[/* the nine SCENARIO_IDS */];   // no pending list: implementation lands with the fixtures
struct WindowTrace { times: Vec<Vec<i64>>, prices: Vec<Vec<f64>> }   // per step, per column; the replay result
impl q_parity::determinism::BitEq for WindowTrace { /* bit_eq over every step and column */ }
fn replay(file: &q_parity::fixture::FixtureFile) -> Result<WindowTrace, String>;
#[test] fn bar_window_reference_gate();      // load_family + check_accounting + replay + compare_f64(Exact) / Int64
#[test] fn bar_window_double_run();          // check_double_run_with(|| replay(file)) per fixture
#[test] fn bar_window_negative_controls();   // one-ULP close, +1 µs time, and a flipped value bit each fail
```

## Implementation decisions

- **No `arrow-rs` dependency. The layout is Arrow-compatible by construction:
  plain `Vec<i64>`/`Vec<f64>`/`Vec<i8>` plus an LSB-first bitmap.** The `arrow`
  crate family (`arrow-array`, `arrow-buffer`, `arrow-data`, `arrow-schema`)
  would add dependencies to every crate above `q-buffers`. That includes `q-qt`,
  whose build already spends its time in `cxx-qt` and the Qt download.
  `arrow-array` also pulls in `ahash` and `hashbrown`, whose default hashing is
  seeded at runtime. That is the class of hazard the determinism lints exist to
  keep out of semantic crates, and it would need auditing before any use. Arrow
  releases a breaking major version roughly every quarter, so pinning it now
  means migrations before anything reads Arrow. No consumer needs it now: the
  kernels read slices, and Python reads numpy. The layout keeps the later
  adoption cheap. `arrow_buffer::Buffer::from_vec` and `ScalarBuffer::from(Vec<T>)`
  take ownership of a `Vec` without copying, and a `BooleanBuffer` wraps
  exactly this bitmap. When `q-io` adds Arrow in Phase 3, the conversion is a
  move, not a transform.

- **Floats use NaN for missing, and no column has a validity buffer.** The Q-022
  kernels define NaN as missing over contiguous float64. A second missing-value
  encoding in the frame would force every kernel to consult two sources, and the
  two could disagree. Arrow permits an absent validity buffer with
  `null_count = 0`, so the layout stays valid Arrow. Boolean and integer columns
  that pandas would hold as object or float because they contain NaN are
  rejected at the binding (`add_column` accepts only numpy `bool`/`int8`/`int64`
  dtypes). A silent `NaN -> False` coercion would change a signal.

- **The three contracted volume columns are `Option<Vec<i64>>`, and there is no
  `volume` bar column.** The contract has `tick_volume`/`spread`/`real_volume` as
  int64. The live window carries none of them (the `ohlcv_list_to_frame` name
  mismatch above), and the goldens use a float `volume`. Making them required
  would fail both. Inventing a reserved `volume` would put an uncontracted name
  into the bar schema. A float `volume` rides as an ordinary derived float64
  column, which keeps the goldens' frame expressible without changing its dtype.

- **Derived columns allow int64 in addition to float64, bool and int8.**
  `bar_index` is `np.arange(len(df), dtype=int)`, which is int64
  (`lai_lau_common.py:14`, `tsmom.py`, `hurst_trend_blend.py`). Storing it as
  float would change a column type the strategies compare with integers, and
  int8 would overflow after 127 bars.

- **Derived columns are a `Vec<(String, Column)>` searched linearly, not a
  `HashMap`.** Iteration order must be deterministic and equal to insertion
  order, which is also pandas' column order and the order a later record-batch
  export must use. Frames carry a few dozen columns at most, so a linear lookup
  costs nothing measurable. It also avoids the hash-ordering hazard.

- **Time is `Vec<i64>` microseconds of bar open time, and `TimeLabel` is a closed
  enum of the contract marker and `UTC`.** Microseconds is the unit of the
  contracted `timestamp[us]`. The frame never converts clocks, because the
  engine's day-trade windows use `timestamp.time()` and `index.date` on
  whatever clock the index holds. Converting would shift session boundaries by
  three hours. The label exists so a caller cannot mistake one clock for the
  other. An open string would allow labels no kernel interprets. The Brasília
  marker string is checked against the vendored `bars.schema.json` in a test, so
  the constant is not an unchecked mirror.

- **The binding accepts `datetime64[us]` and `datetime64[ns]` only, and rejects
  `NaT` and any nanosecond value not divisible by 1,000.** pandas indexes are
  `datetime64[ns]` (the evaluator's `pd.to_datetime` index, and the goldens'
  `date_range`), so rejecting ns would force a cast at every call site. Silent
  truncation would merge two distinct sub-microsecond bars into one open time,
  and the window would then deduplicate them. Plain int64 is not accepted,
  because its unit cannot be known. The tz-aware goldens index is passed as
  `index.tz_convert(None).to_numpy()` with `tz="UTC"`, and Q-028 owns that call
  site.

- **`BarFrame::try_new` requires strictly increasing time, and
  `RollingBarWindow::ingest_completed` is the only place that sorts and
  deduplicates.** Kernels walk bars by index and assume one bar per open time, as
  `engine.py`'s `chunk.iloc[i]` does. A frame that could hold duplicates would
  push that check into every kernel.

- **Window merge algorithm:** if the window is empty or the batch is strictly
  increasing and starts after the newest completed time, append the batch and
  drain the front to the bound. Otherwise, build the concatenation existing ++
  batch, compute a stable sort of positions by time, keep the last position for
  each time, and take the final `bound` rows. This is the pandas sequence
  (`concat`, `duplicated(keep="last")`, `sort_index`, `iloc[-bound:]`) stated
  precisely. "Last" means later in the concatenation, so a batch beats the
  window, and a later row in a batch beats an earlier one. The stable sort is
  what makes that well defined. pandas' `sort_index` does not guarantee
  stability, so fixture scenario 7 records what today's pandas actually does,
  and a disagreement is reported, not patched over. The fast path covers the
  live case (one new bar), so the common ingest is an append and a front drain.

- **Completed bars are stored as one contiguous `BarFrame`, not a per-column ring
  buffer.** The evaluator recomputes indicators over the whole window after
  every ingest, so every ingest is followed by a contiguous read of up to
  `bound` rows. A ring buffer would need that copy on every read, while a
  front drain moves at most `bound` rows per ingest: the same order of cost,
  with a zero-copy `completed()` borrow on the Rust side. For a bound of 605,
  that is at most 605 × 8 bytes per column moved per ingest. This is measured by
  `bench-bar-window`, not assumed.

- **A window's `VolumeSet` and `TimeLabel` are fixed at `new`, and a mismatched
  batch returns `VolumeSetMismatch`/`LabelMismatch`.** `pd.concat` of frames
  with different columns NaN-fills and upcasts int64 to float64. Reproducing that
  would change contracted types, and the evaluator never hits it because every
  batch comes from the same provider.

- **Seeding is `ingest_completed` into an empty window, and there is no separate
  seed operation.** `seed_window` sorts and trims but does not deduplicate. Its
  inputs come from `ohlcv_list_to_frame` over provider bars, which have unique
  open times, so the two agree on every input the worker produces. A frame that
  could hold duplicates would break the strictly-increasing invariant. The
  divergence (duplicate open times in a seed) is recorded here so Q-031 does not
  rediscover it, and no fixture exercises it.

- **Forming-bar rules follow `cursor.py` `bar_transitions`:** a forming bar
  replaces the previous one unconditionally, because detecting "values changed"
  is the publisher's job and would need float equality. A completed ingest
  whose newest time is `>=` the forming time clears the slot: equal is
  promotion, and greater means the forming bar rolled over. `set_forming` with
  a time `<=` the newest completed time returns `StaleForming`. The forming bar
  is stored as a one-row `BarFrame` outside `completed`, so the bound, the
  merge and the completed view cannot include it by accident. The evaluator has
  no forming bar to export, so these rules are unit-tested only.

- **Rolling-window parity uses Q-021 fixtures exported by driving the real
  `StrategyEvaluator`, not a re-typed copy of its pandas lines.** The exporter
  builds `StrategyEvaluator(..., strategy=<stub>, window_bound=N)`. The stub
  strategy returns its input from `compute_indicators` and empty lists from the
  `check_*` methods. The exporter calls `seed_window`/`ingest_completed_bars`
  per scenario step and records `_rolling` after each step. Copying the four
  pandas lines into the exporter would test the copy, and a later q_backend
  change to the window would go unnoticed. Reading the private `_rolling` is
  acceptable in an exporter pinned to one commit. The family is a
  `tools/reference/families/bar_window.py` module registered in `FAMILIES`, not
  a second exporter, so fetching, pin checks, encoding and provenance exist once
  (Q-021). The exporter passes only `open`/`high`/`low`/`close` to the
  evaluator. The live window carries only those columns (see
  `ohlcv_list_to_frame` above), and the generator's float `volume` is not a
  bar column in the frame. Each file's cases are its steps in order. They
  carry the method called, the batch, and the window after the step, so the
  Rust replay needs nothing but the file. These fixtures are bound from the
  moment they land, with no pending entries, because the implementation lands
  in the same task.

- **Values use Q-021's column encoding unchanged: times as `int64` columns,
  prices as `float64` columns written with `repr`, and every column carrying
  `bits_fnv1a64`. The policy is `{"kind": "exact"}`.** Q-021's reader parses
  with `serde_json`'s `float_roundtrip`, and `Column::from_json` verifies the
  bit checksum, so a decimal price already reaches the gate bit-exact. A hex
  encoding would be a second float format that splits the reader and hides
  values from reviewers. `compare_f64` under `Policy::Exact` compares bits
  after NaN canonicalization and keeps the zero sign. `Int64` columns are
  always compared exactly. That is the right policy for a window that only
  moves values. The indicator policy (`abs_tol` 1e-10) exists because pandas
  and Rust compute floats differently, and a window that changed any bit of a
  price has a defect. Times leave the exporter through `encode_time`: the
  goldens generator's UTC index is converted with `tz_convert(None)`, and
  export fails if any nanosecond value is not a whole microsecond. Nothing is
  silently truncated, which matches the binding's own rule.

- **The gate is `crates/q-buffers/tests/bar_window_gate.rs`, built from
  `q-parity` pieces, not `run_gate`.** `run_gate` takes a `ReferenceSet` of
  series-in/series-out `KernelFn`s, while a scenario is an operation sequence
  with state. The test calls `load_family(root, "bar_window")` and
  `check_accounting(&files, BOUND_SCENARIOS, &parse_pending("")?,
  BACKEND_REV)`. That gives the same `Unaccounted`, `BoundUnknown` and
  `ProvenanceRev` rules as the indicator gate. It then replays each file
  through `RollingBarWindow` (a `seed_window` step is an ingest into a new
  window, per the seeding decision) and compares each step with `compare_f64`
  under the file's policy plus the `Int64` comparison. It reports failures as
  `GateFailure::Golden`, so `GateReport::summary` reads the same as the
  indicator gate. Determinism is `check_double_run_with` over a `replay`
  closure that returns a local `WindowTrace` implementing `BitEq`, because the
  trait's built-in impls cover single vectors, not a step sequence. `q-parity`
  is a dev-dependency of `q-buffers`, and `make parity-isolation` keeps proving
  it stays out of `q-py` and `q-qt`.

- **`bar_window` fixtures are exported at the single `BACKEND_REV` the
  indicator family uses. That pin stays before Q-023, the first `q_backend`
  commit that calls `q_core`, and this task does not move it.** One pin file
  and one `ProvenanceRev` check means the two families can never describe two
  different backends. A reference produced by a backend that already
  delegates to `q_core` would compare `q_core` with itself. The evaluator
  window does not call indicators, but the pin is shared, and Q-021's
  `check_imports` guard on the indicator family would reject a post-Q-023 pin
  anyway. If `BACKEND_REV` changes before this task starts, both families are
  regenerated in the same commit.

- **CI does not run `make fixtures-backend-check`. It stays a required local
  check before merging any change to `tools/reference/families/bar_window.py`,
  the `bar_window` fixtures, or `BACKEND_REV`, and the README says so.** The
  check needs `uv sync --frozen` of `q_backend`'s whole lock at
  `BACKEND_REV`. That is 5.3 GB locally, including `torch` and 37 `nvidia-*`
  wheels, on a GitHub runner with about 14 GB of free disk. It would run on
  every push to protect nine fixtures that change only when one of those three
  paths changes. The drift it detects is already caught without the network:
  every `cargo test` compares each fixture's `backend_rev` with `BACKEND_REV`
  (`ProvenanceRev`), so a pin edit without regeneration fails `make check`.
  An exporter edit without regeneration is caught by the required local
  check, and `fixtures-test` unit-tests the family's encoding in the minimal
  environment. Step 8 measures the local sync and export time and reports
  it. If that measurement contradicts the premise, for example a warm-cache
  sync under five minutes and under 6 GB, the handoff says so, and a
  path-filtered CI job becomes a follow-up instead of being added silently.

- **Fixture scenarios**, with bound 20 unless stated, built from the first 40
  rows of `synthetic_ohlcv` as Q-021 AST-extracts it from `test_goldens.py`
  at `BACKEND_REV` (hourly, UTC, seed 20240609):
  1. seed 30 → bars 10..29;
  2. ingest bar 30 → bars 11..30;
  3. re-deliver bar 30 with `close + 1.0` → length 20, and the new close is present;
  4. overlapping batch of bars 28..32 (the `bar_coordinator` `overlap_bars = 1`
     pattern widened) → bars 13..32;
  5. ingest bar 5 into the full window → unchanged;
  6. bound 50, seed bars 0..9 and 12..19, then ingest 10..11 → 20 bars in order;
  7. a batch with order [33, 31, 33', 32], where 33' has different prices → 33'
     wins;
  8. empty batch → unchanged;
  9. bound 1 over scenario 2's inputs → bar 30 only.

- **Frame and column tests compare floats with `Column::bytes_eq` or
  `f64::to_bits`, never `==`.** `float_cmp` is denied workspace-wide, and bitwise
  equality is the property being claimed. `assert_eq!` on `f64` would not
  compile under the lints anyway.

- **The bars schema JSON is vendored into `contracts/schema/api/arrow/` by
  extending `make contracts`, and `contracts-check` diffs it separately from the
  generated Rust tree.** The contracted-columns test must read the contract,
  not a pasted copy (invariant 2). This mirrors Q-017's vendoring of
  `dataset-manifest.schema.json` in `q_backend`. `diff -ru contracts
  generated/rust` gains `--exclude=schema` so the extra directory does not
  register as drift. `#[path]` includes named files only, so the JSON does not
  affect compilation.

- **`numpy = "0.24"` in `q-py`, and `numpy>=2` becomes a runtime dependency of
  the wheel.** The `numpy` crate is versioned in step with `pyo3`, so 0.24
  matches the pinned `pyo3 0.24.2`. `q_backend` already requires
  `numpy>=2.4.4`. Q-022 adds the same crate in parallel, and whichever task
  lands second keeps the existing entry instead of adding a second version.
  `tests/test_wheel.py`'s venv must install numpy before the frame tests run.

- **Reads from Python return copies (`PyArray1::from_slice`, and a bitmap unpack
  for booleans).** A view into `RollingBarWindow` memory would alias bytes that
  the next `ingest_completed` moves. The copies are bounded by the window bound
  (§4.6). The Rust side borrows without copying.

- **Base price columns accept NaN.** The pandas path does not reject a NaN price,
  so rejecting it here would fail an input that today's backtest runs. Validity
  of market data is the ingest path's concern, not the buffer's.

- **Known collision recorded for Q-028:** `gatev_pairs.compute_indicators`
  overwrites `open`/`high`/`low`/`close` and writes a float `spread`
  (`strategies/gatev_pairs.py:159-184`). Reserved names reject that, which is
  correct for a bar frame. The pairs strategy has to express its synthetic
  series as a new frame or as differently named columns when it moves.

## Ordered implementation

1. [x] Work on the branch `Q-025-columnar-bar-frames` in `q_core`, created from
   `development` by `./work start`. Confirm that Q-021's
   `fixtures/reference/`, `tools/reference/export_reference.py` with
   `FAMILIES`, `crates/q-parity`, and the `fixtures-backend` and
   `fixtures-backend-check` targets are present on `development`. Record
   `BACKEND_REV` and confirm it predates Q-023.
2. [x] Extend `make contracts` to copy
   `schema/api/arrow/bars.schema.json` into `contracts/schema/api/arrow/`. In
   `contracts-check`, add `--exclude=schema` to the Rust `diff -ru` and add a
   `diff` of the vendored JSON against the checkout's copy. Run
   `make contracts`, and confirm `make contracts-check` is clean. Then edit the
   vendored JSON, confirm `contracts-check` fails, and revert. Commit.
3. [x] Write failing tests in `q-buffers/src/column.rs`:
   - `Bitmap::from_bools` of `[true, false, true, true, false, false, false,
     false, true, true]` has `as_bytes() == [0b0000_1101, 0b0000_0011]`;
   - `get(9)` is `true`;
   - `to_bools` round-trips;
   - `ColumnType::Float64.arrow_type()` is `"float64"` and `TimestampMicros` is
     `"timestamp[us]"`;
   - `Column::Float64(vec![f64::NAN]).bytes_eq` of itself is `true`;
   - `bytes_eq` of `0.0` against `-0.0` is `false`.

   Confirm they fail, implement, and confirm they pass. Commit.
4. [x] Write failing tests in `frame.rs`:
   - `try_new` with times `[0, 3_600_000_000, 7_200_000_000]` succeeds with
     `len() == 3`;
   - times `[0, 0]` give `NonIncreasingTime { index: 1 }`;
   - a `close` of length 2 against 3 times gives `LengthMismatch` naming
     `close`;
   - `push_column("close", …)` gives `ReservedName`;
   - pushing `"rsi"` twice gives `DuplicateName`;
   - `""` gives `EmptyName`;
   - `column_names()` after pushing `rsi`, `buy_signal` and `bar_index` to a
     frame with `tick_volume` only is `[time, open, high, low, close,
     tick_volume, rsi, buy_signal, bar_index]`;
   - `TimeLabel::parse("America/Sao_Paulo")` gives `UnknownTimeLabel`.

   Confirm they fail, implement, and confirm they pass. Commit.
5. [x] Write a failing contract test in `frame.rs`: load
   `contracts/schema/api/arrow/bars.schema.json` with `serde_json` (test-only
   `include_str!`). Assert that `schema()` of a frame with all three volume
   columns equals the JSON `fields` on `name`, `type`, `nullable` and `tz`, in
   order, and that `TimeLabel::BrasiliaWallclock.contract_marker()` equals the
   `time` field's `tz`. Confirm it fails, implement `schema()`, and confirm it
   passes. Change `close` to `float32` in the vendored copy, confirm the test
   fails, and revert. Commit.
6. [x] Write failing tests in `window.rs` for the merge, one per Q-025 fixture
   scenario above, with hand-built inputs and expected times. Also add:
   - `new(0, …)` gives `ZeroBound`;
   - a batch with `spread: Some` into a window created without spread gives
     `VolumeSetMismatch`;
   - after scenario 3, `completed().close()[19].to_bits()` equals the
     re-delivered value's bits.

   Confirm they fail. Implement the fast path and the stable-sort merge, then
   confirm they pass. Commit.
7. [x] Write failing forming-bar tests in `window.rs`:
   - with completed bars ending at t=10h, `set_forming` at 11h makes
     `forming().time() == [11h]` and `len()` unchanged;
   - a second `set_forming` at 11h with a different close replaces it;
   - `set_forming` at 10h gives `StaleForming`;
   - `ingest_completed` of 11h clears `forming()` and makes 11h the newest
     completed;
   - with forming at 12h, ingesting 13h clears it;
   - with bound 2 and 2 completed bars plus a forming bar, `len()` is 2;
   - `set_forming` with 2 rows gives `FormingNotSingleBar`.

   Confirm they fail, implement, and confirm they pass. Commit.
8. Add exporter tests to `tools/reference/test_export_reference.py`:
   - `encode_time` of `pd.date_range("2023-01-02", periods=2, freq="h",
     tz="UTC")` gives an `int64` column with values
     `[1672617600000000, 1672621200000000]`;
   - an index with a 1 ns remainder raises;
   - `_StubStrategy.check_entry_conditions` returns `[]`;
   - `scenario_steps` yields the nine ids with bounds 20, 20, 20, 20, 20, 50,
     20, 20, 1.

   Confirm they fail under `make fixtures-test`. Implement
   `tools/reference/families/bar_window.py`, register it in `FAMILIES`, and
   set `BACKEND_FAMILIES := bar_window` in the `Makefile`. Confirm the tests
   pass. Run `time make fixtures-backend`, which writes
   `fixtures/reference/bar_window/*.json` at `BACKEND_REV`. Record the sync
   and export wall-clock time and the environment's size on disk
   (`du -sh` of the synced `.venv`). Confirm every file's `backend_rev`
   equals `BACKEND_REV` and `environment` is `"backend"`. Run
   `make fixtures-backend-check` and confirm it is clean. Confirm that
   `make fixtures-check` (numeric families only) is unchanged. Commit.
9. Add `q-parity` as a dev-dependency of `q-buffers`. Write
   `crates/q-buffers/tests/bar_window_gate.rs` with
   `bar_window_reference_gate` and `bar_window_double_run`. Before the window
   adapter is wired, `replay` returns an error, so confirm both tests fail
   with one failure per scenario. Wire `replay` through `RollingBarWindow`
   and confirm both pass. Then write `bar_window_negative_controls` over the
   committed `s03_redeliver` fixture. Each of the following must fail:
   - the expected close at step 2, index 19 (the re-delivered bar), moved by one ULP
     (`f64::from_bits(bits + 1)`), with `MismatchKind::Bits`;
   - an expected time moved by +1 µs, with `MismatchKind::Int64`;
   - a flipped value bit in the loaded JSON, with the checksum error from
     `Column::from_json`;
   - a replay closure that adds a static `AtomicU64` call count to one price
     after its first call, which fails `check_double_run_with`.

   Also, adding a tenth file in a temp copy of the family directory reports
   `Unaccounted`, and the unmodified fixtures pass. If scenario 7 disagrees
   with the unit test from step 6, stop and report pandas' actual order
   instead of editing the fixture. Confirm `make parity-isolation` still
   passes. Commit.
10. Add `numpy = "0.24"` to `q-py`, `numpy>=2` to `pyproject.toml`, and install
    numpy in the `tests/test_wheel.py` venv. Write failing
    `tests/test_bar_frame.py`, run by `make wheel-test` in the same venv:
    - a frame from `synthetic_ohlcv`-shaped arrays with
      `pd.date_range("2023-01-02", periods=5, freq="h").to_numpy()` (ns)
      returns `column("time")` as `datetime64[us]` equal to the input;
    - `column("close").tobytes()` equals the input bytes;
    - `add_column("buy_signal", np.array([True, False, True, False, True]))`
      round-trips as `bool`;
    - `add_column("bar_index", np.arange(5))` round-trips as `int64`;
    - `add_column("dir", np.array([1, 0, -1, 0, 1], dtype=np.int8))`
      round-trips;
    - a time array of `[0, 1] ns` raises `ValueError` mentioning `time`;
    - `NaT` raises;
    - a float16 `open` raises `TypeError` mentioning `open`;
    - an object-dtype `[True, None, …]` column raises `TypeError` naming the
      column;
    - a `RollingBarWindow(20)` fed scenario 2 from Python matches the Rust
      expectation;
    - `q_core.version()` is unchanged.

    Confirm they fail, implement `crates/q-py/src/frame.rs`, and confirm they
    pass. Commit.
11. Verify that builds stay separate: `cargo build -p q-qt` succeeds with no
    Python headers in use, and `make wheel` succeeds without Qt, as in Q-007
    step 8. Run `cargo tree -p q-buffers` and confirm it shows only `serde` and
    `serde_json`. Commit if a manifest change was needed.
12. Add `tests/bench_bar_window.py` and a `bench-bar-window` target that builds
    the wheel and runs the script in a venv with numpy and pandas. The script
    runs five times: 10,000 one-bar ingests into a full 605-bar window through
    `RollingBarWindow.ingest_completed`, against the same through the
    evaluator's pandas sequence (`concat`, `duplicated(keep="last")`,
    `sort_index`, `iloc[-605:]`). It prints per-run and median microseconds per
    ingest for both. The target is not part of `make check`. Commit.
13. Human step, matching human-verifiable criterion 1: run
    `make bench-bar-window` and record per-run and median figures for both
    paths.
14. Human step, matching human-verifiable criterion 2: review the `q-buffers`
    docs against the strategy, genome and exit-rule columns listed in
    current-system context, and list any column without a supported type.
15. Human step, matching human-verifiable criterion 3: in the full backend
    environment, run `make fixtures-backend-check` and confirm there is no
    diff. Record the wall-clock time. This is the local check that stands in
    for CI.
16. Add the README fixture-protocol lines for `bar_window` (backend family;
    `fixtures-backend-check` required locally before merging changes to the
    family, its fixtures or `BACKEND_REV`; not run in CI, and why). Run
    `make check` and commit any fixes.

## Validation

- **Unit:**
  - bitmap packing and trailing bits;
  - column type names;
  - bitwise equality, including NaN and `-0.0`;
  - frame validation for each `FrameError` variant;
  - column order;
  - the contracted schema against the vendored JSON;
  - merge scenarios 1 to 9;
  - forming-bar rules;
  - volume-set and bound rejection.
- **Integration:** numpy round-trips and dtype, `NaT` and precision rejection
  through the installed wheel; a Python-driven window scenario.
- **Regression:**
  - `fixtures/reference/bar_window/` exported from `q_backend`'s
    `StrategyEvaluator` at `BACKEND_REV` (pre-Q-023), compared under
    `Policy::Exact` by `bar_window_gate.rs`, with `ProvenanceRev` checked in
    every `cargo test`;
  - `check_double_run_with` over each scenario replay;
  - `make fixtures-backend-check`, run locally, not in CI;
  - `make fixtures-check` and `make parity-isolation` unchanged;
  - `version()`/`contracts_rev()` wheel test unchanged;
  - `q-qt` harness unchanged;
  - `contracts-check` clean, including the vendored schema.
- **Manual:** step 14 column review.
- **Measurement:** step 13, `RollingBarWindow` against the pandas window update,
  five runs of 10,000 one-bar ingests at a bound of 605, with individual values
  and medians reported. No threshold.
- **Pins:** `CONTRACTS_REV` is unchanged, and the vendoring scope grows by
  `schema/api/arrow/bars.schema.json`. Update the `q_core` row in
  `q_contracts/COMPAT.md` only if the pin moves.

```bash
cd /home/gui/projects/q/q_core
make contracts-check
make fixtures-test
cargo test -p q-buffers           # unit tests + tests/bar_window_gate.rs
make parity-isolation
make wheel-test                   # includes tests/test_bar_frame.py
cargo tree -p q-buffers --depth 1
make check

# negative controls, run by hand and reverted
sed -i 's/"float64"/"float32"/' contracts/schema/api/arrow/bars.schema.json && cargo test -p q-buffers contract; git checkout -- contracts/

# backend family (multi-GB q_backend environment; local only, not CI)
time make fixtures-backend-check

# human
make bench-bar-window
cargo doc -p q-buffers --no-deps --open
```

## Handoff

Report `BACKEND_REV`, and confirm it is the same pin as the indicator family
and predates Q-023. Report the nine scenarios with their resulting window
lengths and first and last open times. Say explicitly whether scenario 7
(in-batch duplicates) matched pandas on the first run. Report the
`make fixtures-backend` sync and export wall-clock time, the synced
environment's size, and whether those numbers support keeping
`fixtures-backend-check` out of CI. Report each negative control's failure
message: the one-ULP close, the +1 µs time, the flipped bit, the counter
closure, the `Unaccounted` extra file, the edited vendored schema, and the
edited `contracts-check` input. Report `cargo tree -p q-buffers --depth 1` output to
show that no Arrow crate was added, and the `numpy` crate version resolved in
`Cargo.lock`. Say whether Q-022 had already added it. Report the wheel's new
runtime dependency line, and confirm that `q-qt` built without Python and the
wheel built without Qt. From the human steps, report per-run and median
per-ingest microseconds for `RollingBarWindow` and for the pandas sequence,
and list any strategy, genome or exit-rule column that the review found
unrepresentable.
