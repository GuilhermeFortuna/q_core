# Q-021 implementation plan: Reference fixtures and parity gate

**Status:** authoritative in the [Q project board](https://github.com/users/GuilhermeFortuna/projects/2)  
**Specification:** [`../specs/Q-021-reference-fixtures-and-parity-gate-spec.md`](../specs/Q-021-reference-fixtures-and-parity-gate-spec.md)  
**Depends on:** Q-007

## Current-system context

`q_core` at `v2026.09.12` (`84bdeb0`) has six crates. None has semantics. Each
semantic crate has `#![forbid(unsafe_code)]`, one `CRATE_NAME` constant, and
one unit test. `[workspace.lints.clippy]` in `Cargo.toml` denies `float_cmp`,
`float_cmp_const`, `imprecise_flops`, `suboptimal_flops`, `disallowed_types`,
and `disallowed_methods`, with `clippy.toml` naming `Instant`, `SystemTime`,
`env::var`, and `env::var_os`. `make lint` runs clippy with `--all-targets`, so
test code is linted too. `make check` runs `fmt-check lint test wheel-test
qt-test contracts-check`, and CI runs exactly that with Rust 1.98.0 and `uv`.
`serde` and `serde_json` 1.0.151 are already in `Cargo.lock` through
`q-buffers`, and `serde_json`'s `float_roundtrip` feature is available but
unused. `make contracts` and `make contracts-check` show a pinned-fetch
pattern to reuse: `CONTRACTS_REV`, a `CONTRACTS_REPO ?=` override for offline
clones, a temp directory, and `diff -ru`. `tests/test_wheel.py` is the only
Python in the repository and runs under `uv`. There is no fixture directory,
no tools directory, and no test-support crate.

The reference lives in `q_backend` at `origin/development` `144345a`. The
three modules are `src/q_backend/backtesting/technical_indicators.py`
(`compute_realized_vol`, `compute_yang_zhang`, `compute_rsi`,
`compute_bollinger_bands`, `compute_macd`, `compute_donchian_channels`,
`compute_atr`), `backtesting/moving_averages.py` (`compute_ma` over
`VALID_MA_TYPES`, with `_wma` and `_hma`), and `backtesting/transforms.py`
(`compute_rolling_zscore`, `compute_rolling_rank`, `compute_pct_change`,
`compute_clip`). All three import only `numpy`, `pandas`, `typing`, and
`__future__`. The package `backtesting/__init__.py` imports the engine, so the
modules cannot be imported normally without the whole backend.
`tests/backtesting/test_goldens.py:84` defines `synthetic_ohlcv(n=400,
seed=20240609, freq="h", start="2023-01-02")`. The module imports the engine,
evaluator, and tick runner at the top level. `FLOAT_DECIMALS = 10` (line 70)
and `_normalize` (line 132) round floats and collapse `-0.0` (line 143).
`test_determinism_double_run` (line 460) and
`test_tamper_produces_readable_diff` (line 467) are the precedents for the
double-run check and the negative control. `src/q_backend/features/leakage.py`
`assert_causal` recomputes on `bars.iloc[: t + 1]` at sampled indices with
`rtol = atol = 1e-9`, and it imports only `pandas` and the standard library at
runtime. The backend's lock pins `numpy 2.4.4`, `pandas 3.0.2`,
`python-dateutil 2.9.0.post0`, and `tzdata 2026.2`, with no `bottleneck` or
`numexpr`. The full environment includes `torch` and 37 `nvidia-*` wheels,
and the local `.venv` is 5.3 GB. Checked in that environment: `0.0/0.0` gives
the NaN bit pattern `0xfff8000000000000`. `compute_rsi` returns NaN on a
constant series and `100.0` on an increasing one. `compute_yang_zhang(window=1)`
and `compute_rsi(period=0)` raise `ZeroDivisionError`,
`compute_rolling_rank(window=0)` raises `IndexError`, and
`compute_donchian_channels(period=0)` returns all NaN. `q_core` has nothing
that knows any of these facts. This task records them and builds the gate
that holds Rust kernels to them.

## Interfaces produced

```
// q_core/ (new and changed files)
BACKEND_REV                                 q_backend commit hash the reference fixtures are exported from
tools/reference/pyproject.toml              minimal exporter environment: numpy==2.4.4, pandas==3.0.2 (exact pins)
tools/reference/uv.lock
tools/reference/export_reference.py         THE exporter: CLI, shared envelope/encoding, family registry
tools/reference/families/indicators.py      the indicator family (environment "numeric")
tools/reference/test_export_reference.py    stdlib unittest suite for the exporter
fixtures/reference/                         THE fixture root; one subdirectory per family
fixtures/reference/inputs/<input_id>.json   shared input series (any family may reference them)
fixtures/reference/indicators/<function_id>.json   one fixture per function
                                            (Q-025 adds fixtures/reference/bar_window/<scenario_id>.json
                                             and tools/reference/families/bar_window.py, environment "backend")
crates/q-parity/                            test-support crate; publish = false; dev-dependency only
crates/q-indicators/Cargo.toml              [dev-dependencies] q-parity
crates/q-indicators/tests/reference_gate.rs the indicator gate: BINDINGS table (empty) + run
crates/q-indicators/tests/reference_pending.txt   16 function ids, one per line
Makefile                                    fixtures, fixtures-check, fixtures-backend, fixtures-backend-check,
                                            fixtures-test, parity-isolation; check extended
README.md                                   q-parity row, dependency direction, fixture protocol section
q_contracts: COMPAT.md                      "Reference fixture pins" section with q_core's BACKEND_REV
```

```
// function ids (file stems; also the pending-list entries)
realized_vol  yang_zhang  rsi  bollinger_bands  macd  donchian_channels  atr
ma_sma  ma_ema  ma_smma  ma_wma  ma_hma
rolling_zscore  rolling_rank  pct_change  clip

// input ids
synthetic_ohlcv_n400   generator defaults, from test_goldens.py at BACKEND_REV
constant_n60           open = high = low = close = 100.0, volume = 1000.0
monotonic_up_n60       close = 100 + i; open = close - 0.5; high = close + 0.25; low = open - 0.25; volume = 1000.0
nan_gaps_n120          synthetic_ohlcv_n400 rows 0..120 with NaN at rows 0, 17, 18, 19, 64 in every column
short_n3               synthetic_ohlcv_n400 rows 0..3
```

```jsonc
// fixture layout (format "q-core-reference-fixture/1"); keys sorted, indent 1, one array element per line
// fixtures/reference/inputs/<input_id>.json
{ "format": "...", "input_id": "nan_gaps_n120",
  "provenance": { "backend_repo": "...", "backend_rev": "<40 hex>", "exporter": "tools/reference/export_reference.py",
                  "construction": "<one sentence>", "generator": { "path": "tests/backtesting/test_goldens.py",
                  "blob": "<git blob id>", "name": "synthetic_ohlcv", "kwargs": { "n": 400, "seed": 20240609, ... } } },
  "columns": { "close": { "dtype": "float64", "bits_fnv1a64": "0x…", "values": [ 100.1, null, "inf", -0.0, ... ] }, ... } }

// every family file shares this envelope; "cases" layout is family-specific
{ "format": "q-core-reference-fixture/1", "family": "<family>", "fixture_id": "<file stem>",
  "policy": { ... }, "provenance": { "backend_repo", "backend_rev", "exporter", "environment": "numeric" | "backend",
  "python", "numpy", "pandas", "cpu_level", "sources": [ { "path", "blob" } ] }, "cases": [ ... ] }

// fixtures/reference/indicators/<function_id>.json  (family "indicators": series in -> series out)
{ "format": "...", "family": "indicators", "fixture_id": "rsi",
  "policy": { "kind": "abs_rel_tol", "abs": 1e-10, "rel": 1e-12 },   // or { "kind": "exact" }
  "provenance": { "backend_repo", "backend_rev", "source": { "path", "blob", "callable", "ma_type"? },
                  "exporter", "python", "numpy", "pandas", "cpu_level", "causality_checked_indices": <count> },
  "cases": [ { "case_id": "synthetic_ohlcv_n400/period=14",
               "inputs": { "close": "synthetic_ohlcv_n400.close" },
               "params": { "period": 14 },
               "expected": { "outputs": { "rsi": { "dtype", "bits_fnv1a64", "values" } } }
                        // or { "rejected": { "python_exception": "ZeroDivisionError" } }
             } ] }
```

```python
# tools/reference/export_reference.py
FORMAT: Final = "q-core-reference-fixture/1"
ABS_TOL: Final = 1e-10
REL_TOL: Final = 1e-12
CANONICAL_NAN_BITS: Final = 0x7FF8000000000000
LOCKED_PACKAGES: Final = ("numpy", "pandas", "python-dateutil", "tzdata")

@dataclass(frozen=True)
class BackendSource:
    checkout: Path          # a checkout whose HEAD is rev
    rev: str                # 40-hex commit
    repo_url: str

@dataclass(frozen=True)
class FunctionSpec:
    function_id: str                       # file stem
    module_path: str                       # backend-relative source path
    callable_name: str
    input_columns: tuple[str, ...]         # positional argument order, e.g. ("high", "low", "close")
    outputs: tuple[str, ...]               # names for returned Series / tuple members, in return order
    param_grid: tuple[dict[str, object], ...]
    fixed_kwargs: dict[str, object]        # e.g. {"ma_type": "hma"}

def fetch_backend(repo_url: str, rev: str, workdir: Path) -> BackendSource: ...   # git init + fetch --depth 1 by hash
def open_backend_checkout(path: Path, rev: str) -> BackendSource: ...             # HEAD == rev and clean tree, else exit 2
def git_blob_id(source: BackendSource, rel_path: str) -> str: ...
def verify_environment(source: BackendSource) -> dict[str, str]: ...              # installed == backend uv.lock, else exit 2
def cpu_feature_level() -> str: ...   # "x86-64-v2" | "x86-64-v3" | "x86-64-v4" | "aarch64" | ...; from /proc/cpuinfo flags + platform.machine()
def check_imports(module_source: str, rel_path: str) -> None: ...                 # stdlib | numpy | pandas outside TYPE_CHECKING
def load_reference_module(source: BackendSource, rel_path: str) -> ModuleType: ...
def load_generator(source: BackendSource, rel_path: str, name: str) -> Callable[..., pd.DataFrame]: ...  # ast-extracted def
def build_inputs(generator: Callable[..., pd.DataFrame]) -> dict[str, pd.DataFrame]: ...
def function_specs() -> tuple[FunctionSpec, ...]: ...
def run_case(spec: FunctionSpec, module: ModuleType, frame: pd.DataFrame, params: dict[str, object]) -> dict[str, object]: ...
def check_reference_causal(spec: FunctionSpec, module: ModuleType, frame: pd.DataFrame,
                           params: dict[str, object], leakage: ModuleType) -> int: ...  # returns indices checked
def fnv1a64_float64(values: np.ndarray) -> int: ...
def encode_column(values: np.ndarray) -> dict[str, object]: ...
def dumps_fixture(payload: dict[str, object]) -> str: ...
class Family(Protocol):
    name: str                                  # subdirectory of the fixture root
    environment: Literal["numeric", "backend"] # "backend" = q_backend's full locked env at BACKEND_REV
    policy: dict[str, object]                  # envelope "policy"
    def export(self, source: BackendSource, out_dir: Path) -> list[str]: ...   # writes <out>/<name>/*.json, returns ids

FAMILIES: Final[dict[str, Family]]             # {"indicators": IndicatorFamily()}; Q-025 registers "bar_window"

def detect_environment(source: BackendSource) -> Literal["numeric", "backend"]: ...  # "backend" iff q_backend importable
def envelope(family: Family, fixture_id: str, source: BackendSource,
             sources: list[str], cases: list[dict[str, object]]) -> dict[str, object]: ...
def compare_trees(committed: Path, regenerated: Path) -> int: ...
    # same cpu_level: byte-for-byte file diff. Different cpu_level: prints both levels; everything
    # but cpu_level and value arrays byte-for-byte, outputs under each fixture's policy, inputs exact.
    # Exit 1 with a unified-diff-style report on any failure.
def main(argv: list[str] | None = None) -> int: ...
    # subcommand "compare COMMITTED REGENERATED" runs compare_trees
    # --family NAME (repeatable; default: all families of the detected environment)
    # --backend-repo URL --rev-file PATH | --backend-checkout DIR ; --out DIR
    # exits 2 if a requested family's environment differs from detect_environment()
```

```python
# tools/reference/families/indicators.py
class IndicatorFamily:          # name "indicators", environment "numeric", policy abs_rel_tol abs 1e-10 rel 1e-12
    def export(self, source: BackendSource, out_dir: Path) -> list[str]: ...
# function_specs, build_inputs, run_case, check_reference_causal, check_imports, load_reference_module,
# load_generator live here (signatures above); fetch/verify/encoding stay in export_reference.py
```

```rust
// crates/q-parity/src/lib.rs
#![forbid(unsafe_code)]
//! Responsibility: test support for the parity gate. Reads reference fixtures, compares
//! outputs under a declared policy, and checks double-run determinism and prefix causality.
//! Dev-dependency only; never linked into q-py or q-qt.
pub mod fixture;
pub mod compare;
pub mod determinism;
pub mod causality;
pub mod gate;

// crates/q-parity/src/fixture.rs
pub const FORMAT: &str = "q-core-reference-fixture/1";

pub enum ColumnData { Float64(Vec<f64>), Int64(Vec<i64>) }
pub struct Column { pub data: ColumnData, pub bits_fnv1a64: u64 }   // checksum verified on load

pub enum ParamValue { Int(i64), Float(f64), Str(String) }
pub type Params = std::collections::BTreeMap<String, ParamValue>;

pub struct ColumnRef { pub input_id: String, pub column: String }   // "synthetic_ohlcv_n400.close"

pub enum Expected {
    Outputs(std::collections::BTreeMap<String, Column>),
    Rejected { python_exception: String },
}

pub struct Case {
    pub case_id: String,
    pub inputs: std::collections::BTreeMap<String, ColumnRef>,   // argument name -> input column
    pub params: Params,
    pub expected: Expected,
}

pub struct Provenance { pub backend_repo: String, pub backend_rev: String, pub source_path: String,
                        pub source_blob: String, pub exporter: String, pub numpy: String, pub pandas: String,
                        pub cpu_level: String }

pub struct FunctionFixture { pub function_id: String, pub policy: crate::compare::Policy,
                             pub provenance: Provenance, pub cases: Vec<Case> }

pub struct InputSet { pub input_id: String, pub backend_rev: String,
                      pub columns: std::collections::BTreeMap<String, Column> }

pub struct ReferenceSet {
    pub inputs: std::collections::BTreeMap<String, InputSet>,
    pub functions: std::collections::BTreeMap<String, FunctionFixture>,
}

pub enum FixtureError { Io { path: PathBuf, message: String }, Parse { path: PathBuf, message: String },
                        UnknownFormat { path: PathBuf, found: String },
                        Checksum { path: PathBuf, column: String, stored: u64, computed: u64 },
                        DanglingInput { path: PathBuf, case_id: String, reference: String } }

/// Family-agnostic view of one file: envelope validated, columns inside "cases" left as JSON
/// for the family's own gate to decode with Column::from_json.
pub struct FixtureFile { pub path: PathBuf, pub family: String, pub fixture_id: String,
                         pub policy: crate::compare::Policy, pub provenance: Provenance,
                         pub cases: Vec<serde_json::Value> }
impl Column { pub fn from_json(value: &serde_json::Value) -> Result<Column, String>; }   // checksum verified
pub fn load_family(root: &Path, family: &str) -> Result<Vec<FixtureFile>, FixtureError>;  // sorted by fixture_id
pub fn load_inputs(root: &Path) -> Result<BTreeMap<String, InputSet>, FixtureError>;
/// The "indicators" family (and any other series-in/series-out family) typed on top of load_family.
pub fn load_reference_set(root: &Path, family: &str) -> Result<ReferenceSet, FixtureError>;
pub fn fnv1a64_float64(values: &[f64]) -> u64;   // NaN canonicalised to 0x7ff8000000000000
pub fn fnv1a64_int64(values: &[i64]) -> u64;

// crates/q-parity/src/compare.rs
pub enum Policy {
    AbsRelTol { abs: f64, rel: f64 },   // NaN positional, inf exact with sign, -0.0 == 0.0,
                                        // finite: |a - e| <= abs  OR  |a - e| <= rel * |e|
    Exact,                     // identical bits after NaN canonicalisation; zero sign significant
}
pub enum MismatchKind { Length { expected: usize, actual: usize }, MissingOutput, UnexpectedOutput,
                        NanPlacement, Infinity, Magnitude { abs_diff: f64 }, Bits, Int64 }
pub struct Mismatch { pub output: String, pub index: usize, pub expected: String, pub actual: String,
                      pub kind: MismatchKind }
pub fn compare_f64(policy: &Policy, expected: &[f64], actual: &[f64]) -> Result<(), (usize, MismatchKind)>;
pub fn compare_outputs(policy: &Policy, expected: &BTreeMap<String, Column>,
                       actual: &KernelOutputs) -> Result<(), Mismatch>;

// crates/q-parity/src/gate.rs
pub struct KernelInputs<'a> { /* argument name -> &'a [f64] */ }
impl<'a> KernelInputs<'a> {
    pub fn column(&self, name: &str) -> Result<&'a [f64], KernelError>;
    pub fn len(&self) -> usize;
    pub fn prefix(&self, len: usize) -> KernelInputs<'a>;
}
pub struct KernelOutputs(pub BTreeMap<String, Vec<f64>>);
pub struct KernelError { pub message: String }
pub type KernelFn = fn(&KernelInputs<'_>, &Params) -> Result<KernelOutputs, KernelError>;

pub struct Binding { pub function_id: &'static str, pub kernel: KernelFn }

pub struct Pending { /* ordered set of function ids */ }
pub fn parse_pending(text: &str) -> Result<Pending, String>;   // one id per line; '#' comments; duplicates rejected

pub enum GateFailure {
    Unaccounted { function_id: String },
    PendingButBound { function_id: String },
    PendingUnknown { function_id: String },
    BoundUnknown { function_id: String },
    DuplicateBinding { function_id: String },
    ProvenanceRev { file: String, recorded: String, pinned: String },
    Golden { function_id: String, case_id: String, mismatch: Mismatch },
    AcceptedRejectedCase { function_id: String, case_id: String, python_exception: String },
    UnexpectedError { function_id: String, case_id: String, message: String },
    Nondeterministic { function_id: String, case_id: String, output: String, index: usize },
    NonCausal { function_id: String, case_id: String, output: String, index: usize, full: String, prefix: String },
}
pub struct GateReport { pub bound: Vec<String>, pub pending: Vec<String>, pub failures: Vec<GateFailure> }
impl GateReport {
    pub fn summary(&self) -> String;   // "reference gate: N bound, M pending, K failures" + one line per failure
    pub fn assert_passed(&self);       // panics with summary() when failures is non-empty
}
pub fn run_gate(reference: &ReferenceSet, bindings: &[Binding], pending: &Pending, pinned_backend_rev: &str) -> GateReport;

/// Bookkeeping shared by every family gate: Unaccounted, PendingButBound, PendingUnknown,
/// BoundUnknown, DuplicateBinding, ProvenanceRev. run_gate calls it; Q-025's bar_window gate
/// calls it with its scenario ids and then does its own replay + compare_* + check_double_run_with.
pub fn check_accounting(files: &[FixtureFile], bound_ids: &[&str], pending: &Pending,
                        pinned_backend_rev: &str) -> Vec<GateFailure>;

// crates/q-parity/src/determinism.rs (generic form, for non-series families)
pub fn check_double_run_with<T, F>(run: F) -> Result<(), String>
where F: Fn() -> T, T: BitEq;          // run twice, compare bit for bit
pub trait BitEq { fn bit_eq(&self, other: &Self) -> Result<(), String>; }   // impls: Vec<f64>, Vec<i64>, KernelOutputs

// crates/q-parity/src/determinism.rs
pub fn check_double_run(kernel: KernelFn, inputs: &KernelInputs<'_>, params: &Params) -> Result<(), (String, usize)>;

// crates/q-parity/src/causality.rs
pub fn check_prefix_causal(kernel: KernelFn, inputs: &KernelInputs<'_>, params: &Params,
                           policy: &Policy) -> Result<(), NonCausalAt>;
pub struct NonCausalAt { pub output: String, pub index: usize, pub full: f64, pub prefix: f64 }
```

```rust
// crates/q-indicators/tests/reference_gate.rs
const BINDINGS: &[q_parity::gate::Binding] = &[];   // Q-022 adds one entry per kernel
const PENDING: &str = include_str!("reference_pending.txt");
const BACKEND_REV: &str = include_str!("../../../BACKEND_REV");
#[test] fn indicator_reference_gate();
```

```
// Makefile (new targets; bodies follow the contracts targets' shape)
BACKEND_REPO ?= https://github.com/GuilhermeFortuna/q_backend.git
NUMERIC_FAMILIES  := indicators            # Q-025 does not add bar_window here
BACKEND_FAMILIES  :=                       # Q-025 sets: bar_window
fixtures:                 uv run --frozen --project tools/reference python tools/reference/export_reference.py $(NUMERIC_FAMILIES:%=--family %) --out fixtures/reference
fixtures-check:           same export into a mktemp directory, then export_reference.py compare fixtures/reference <tmp>
fixtures-backend:         fetch q_backend at BACKEND_REV, uv sync --frozen --project <checkout>, then
                          uv run --frozen --project <checkout> python tools/reference/export_reference.py
                          --backend-checkout <checkout> $(BACKEND_FAMILIES:%=--family %) --out fixtures/reference
                          (no-op with a message while BACKEND_FAMILIES is empty)
fixtures-backend-check:   fixtures-backend into a mktemp directory, then diff -ru per backend family directory
fixtures-test:            uv run --frozen --project tools/reference python -m unittest discover -s tools/reference
parity-isolation:  cargo tree -p q-py -e normal,build and -p q-qt -e normal,build must not list q-parity
check: fmt-check lint test fixtures-test fixtures-check parity-isolation wheel-test qt-test contracts-check
```

```
// dependency direction after this task (README)
q-parity      ───►  serde, serde_json (float_roundtrip); no q-* crate
q-indicators  ───►  (none)            [dev: q-parity]
... unchanged rows ...
```

## Implementation decisions

- **Fixtures are JSON, one array element per line, with NaN as `null` and
  infinities as the strings `"inf"` and `"-inf"`.** JSON is the only format
  that meets all four needs. `q-buffers` already carries `serde_json`, so the
  Rust reader adds no crate. The backend's golden files are JSON, so reviewers
  already read this idiom. One value per line makes a one-value change a
  one-line diff. Python's `json` writes floats with `repr`, which is the
  shortest string that round-trips. CSV has no place for provenance, policy,
  or rejections. Parquet and Arrow IPC need the arrow stack and are not
  diffable. `.npy` is binary. Hex-float bit strings are exact but a reviewer
  cannot see that RSI is 100. The exporter writes with `allow_nan=False`
  after its own mapping, so a stray NaN raises instead of emitting the
  invalid token `NaN`.

- **The Rust reader enables `serde_json`'s `float_roundtrip` feature, and
  every column carries an FNV-1a 64 checksum of its little-endian bits.**
  Without `float_roundtrip`, `serde_json` parses some 17-digit decimals one
  ULP away, and the gate would compare kernels against numbers the reference
  never produced. The checksum makes that failure loud even if a future
  dependency change drops the feature. FNV-1a is used because it is about
  ten lines in each language and needs no dependency. NaN is canonicalized to
  `0x7ff8000000000000` before hashing. The checksum cannot keep payload bits,
  since JSON `null` loses them, and pandas produces `0xfff8000000000000` from
  `0/0`. The Python and Rust test suites both assert that
  `[1.0, -0.0, NaN, +inf]` hashes to `0x892eab94389f5cb0` and `[]` to
  `0xcbf29ce484222325`, so the two implementations cannot drift. The checksum
  is written as a hex string because it exceeds JSON's safe integer range.

- **Inputs are stored once per input set and referenced by
  `"<input_id>.<column>"`, and fixtures contain no timestamps.** Sixteen copies
  of a 400-row OHLCV series would be most of the bytes and most of the review
  noise. All 16 functions are positional, and none reads the index, so a time
  column would be unused data that invites someone to depend on it. Q-025's bar
  times are a separate family and use the `int64` column type.

- **Provenance records `cpu_level` from `cpu_feature_level()`: the highest
  x86-64 psABI level whose flags `/proc/cpuinfo` lists (v2, v3, v4), or
  `platform.machine()` elsewhere.** numpy dispatches `np.log`, used by
  `realized_vol` and `yang_zhang`, to AVX-512 kernels that can differ by one
  ULP from the AVX2 path. The fixtures are therefore a property of the
  machine that exported them as well as of the commit, and a reviewer needs
  to see which machine that was. `compare_trees` keeps the staleness check
  byte-exact when the levels match, which is the local case and the one that
  catches hand edits. When they differ, as they may on a GitHub runner, it
  compares values under the fixture's own policy instead of failing on a
  one-ULP difference that is not staleness. Inputs, provenance, case lists,
  and rejections still match byte for byte, so a pin change or an exporter
  change is still caught. The cost is that a hand edit within tolerance goes
  unnoticed on a machine of a different level. It is caught on the
  exporting machine's level, and README states this.

- **The exporter lives in `q_core` at `tools/reference/`, in its own `uv`
  project pinned to `numpy==2.4.4` and `pandas==3.0.2`. It fetches `q_backend`
  by commit hash into a temp directory, as `make contracts` does.** A `uv` git
  dependency on `q-backend` would resolve the backend's whole dependency set,
  `torch` and 37 `nvidia-*` wheels included, which is 5.3 GB locally, on
  every CI run, to call three pandas modules. A sibling path is forbidden.
  `verify_environment` parses the fetched `uv.lock` and exits 2 if any of
  `LOCKED_PACKAGES` differs from what is installed. Without that check, the
  pins in `tools/reference/pyproject.toml` would be a copy that silently goes
  stale when the pin moves. The backend lock has no `bottleneck` or `numexpr`,
  which are the only optional libraries pandas would route these operations
  through, so the minimal environment computes the same way. Human criterion
  1 confirms this byte for byte against the full environment.
  `--backend-checkout` exists for that one comparison, and it requires the
  checkout's `HEAD` to equal `BACKEND_REV` and the tree to be clean.

- **There is one exporter (`tools/reference/export_reference.py`), one fixture
  root (`fixtures/reference/`), and one gate library (`q-parity`). Families
  plug into them. Each family declares `environment = "numeric"` or
  `"backend"`, and the exporter exits 2 when it is started in the other one.**
  Q-025's nine `bar_window` scenarios drive the real `StrategyEvaluator`.
  `execution/evaluator.py` imports `pydantic` and most of
  `q_backend.backtesting` and `q_backend.execution`, so that family cannot run
  in the minimal environment. It runs under
  `uv run --frozen --project <checkout at BACKEND_REV>`, with `q_backend`
  imported normally. The indicator family must not run there as its only
  check, because CI would then need the 5.3 GB environment for three pandas
  modules. A second exporter for the evaluator would duplicate fetching, pin
  checks, encoding, and provenance, and those copies would drift. Splitting by
  declared environment keeps one of each, and `detect_environment` (whether
  `import q_backend` succeeds) makes running a family in the wrong environment
  impossible rather than merely discouraged. The indicator import check
  applies only to the indicator family. A backend family imports the backend
  on purpose, and its provenance lists the source blobs it drove.
  `make fixtures-check` covers only numeric families and runs in
  `make check`. `make fixtures-backend-check` covers backend families, and
  README requires it before merging a change to those families or to
  `BACKEND_REV`. Whether CI runs it is Q-025's decision against its measured
  sync time. The offline `ProvenanceRev` check in every gate covers both
  kinds in every `cargo test`.

- **The envelope is shared, and `cases` are family-specific JSON.** The
  indicator family types its cases as `ReferenceSet` (series in, series out,
  `KernelFn`). A scenario family types its cases as operation sequences with
  an expected state after each step. `load_family` validates the envelope and
  leaves `cases` as `serde_json::Value`. `Column::from_json` decodes and
  checksums any column a family embeds. `check_accounting` gives every family
  the same bound or pending rules. `compare_f64` with `Policy::Exact` and the
  always-exact `Int64` comparison give Q-025 its no-rounding comparison.
  `check_double_run_with` gives it determinism over a replay closure. Q-025
  should store times as `int64` columns and prices as ordinary float64
  columns. A decimal `repr` parsed with `float_roundtrip` and verified by the
  bit checksum is already bit-exact, so the hex bit strings its draft plan
  mentions are unnecessary, and a second float encoding would split the
  reader.

- **Reference modules are loaded by file path with
  `importlib.util.spec_from_file_location`, after `check_imports` has walked
  their AST.** A normal import runs `q_backend/backtesting/__init__.py`, which
  imports the engine and the rest of the backend. The import check allows the
  standard library (`sys.stdlib_module_names`), `numpy`, and `pandas`, and
  ignores imports inside `if TYPE_CHECKING:`. That is exactly the set
  `leakage.py` and the three modules use today. It fails on anything else,
  including `q_core`. It also enforces the spec's rule that the pin can never
  move to a backend whose indicators delegate to `q_core` (Q-023), because
  such a module has to import it.

- **`synthetic_ohlcv` is taken from `test_goldens.py` by AST extraction of that
  one `FunctionDef`, executed in a namespace holding only `np` and `pd`.**
  Importing the test module imports the engine, evaluator, and tick runner. A
  copy of the generator in the exporter would drift silently when the backend
  test changes. Extraction uses the pinned file's own text, and the fixture
  records its git blob id. If the function gains a dependency, the exec fails
  with a `NameError` at export time instead of producing different data.

- **The exporter runs the backend's own `assert_causal` (loaded from
  `features/leakage.py`) on every non-rejected case before writing. It checks
  every index for inputs of 120 rows or fewer, and for the 400-row series it
  checks indices 0 to 120 plus every 10th index after that and 399.** A
  non-causal reference committed as truth would make every correct kernel
  fail causality while passing goldens. That would look like a harness bug.
  Using the backend's assertion rather than a new one means the reference
  meets the same bar it meets in the backend. The sampling bounds the cost of
  pandas `rolling.apply` inside `_wma`, which is called three times per HMA
  prefix. The Rust side checks every index, because a Rust prefix run costs
  microseconds. The number of indices checked is recorded in provenance.

- **Parameter grids, applied to every input set:** `realized_vol` window {1,
  2, 20, 60} with `periods_per_year` 252; `yang_zhang` window {1, 2, 20, 60};
  `rsi` and `atr` period {0, 1, 2, 14, 100}; `bollinger_bands` (period,
  num_std) {(1, 2.0), (2, 1.0), (20, 0.0), (20, 2.0), (20, 2.5)}; `macd`
  {(1, 1, 1), (3, 6, 2), (12, 26, 9), (26, 12, 9)}; `donchian_channels` period
  {0, 1, 20, 100}; each `ma_*` period {0, 1, 2, 5, 20, 21, 100};
  `rolling_zscore` window {1, 2, 20, 100}; `rolling_rank` window {0, 1, 20,
  100}; `pct_change` `change_bars` {0, 1, 5, 100}; `clip` (low, high)
  {(95.0, 105.0), (100.0, 100.0), (105.0, 95.0), (-1e300, 1e300)}. Each grid
  holds the common value, the smallest valid value, a value longer than
  `constant_n60`, and the value where Python raises or degenerates. These are
  the places where a reimplementation most often differs: HMA's
  `period // 2` and `int(sqrt(period))` floors at 21, Wilder smoothing with
  `alpha=1`, MACD with fast slower than slow, and Donchian's `shift(1)` at
  period 0. The grid lives in `function_specs()`, so extending it is a
  reviewed exporter change followed by a regeneration.

- **A raising case is recorded as `rejected` with the Python exception's
  class name, and the gate requires only that the kernel return an error.**
  Python's `ZeroDivisionError` and `IndexError` for these cases are accidents
  of the implementation, not a contract. Q-022 should reject the parameter
  explicitly. Requiring matching messages would force Rust to imitate those
  accidents.

- **The indicator fixtures declare `{"kind": "abs_rel_tol", "abs": 1e-10,
  "rel": 1e-12}`. A finite value passes if `|a - e| <= abs` or
  `|a - e| <= rel * |e|`. NaN placement and infinity signs are exact under
  every policy, and `-0.0` equals `0.0` only under `abs_rel_tol`.** This
  replaces an absolute-only policy, by human decision, because the WMA and HMA
  references come from numpy's BLAS dot product in `_wma`, and its summation
  order depends on the CPU. Q-022 measured Rust differing from it by up to
  `7.1e-14` at price level ~100. At WIN$N scale (~130,000) the same relative
  gap approaches `1e-10`, so an absolute-only bound would fail correct kernels
  spuriously on real-scale data. The relative bound `1e-12` sits well above
  that measured gap (`7.1e-16` relative) and scales with price. The absolute
  bound `1e-10` is the resolution of the backend's golden files
  (`FLOAT_DECIMALS = 10`), so values near zero (returns, z-scores, histogram
  crossings), where `rel * |e|` collapses, are still held to the goldens'
  10-decimal meaning. Comparing rounded values would be flaky, because two
  values one ULP apart can round to different tenth decimals. A tolerance has
  no such boundary. The `Exact` policy keeps the zero sign because it is for copied
  values (Q-025), where a sign flip is a defect. The policy lives in the
  fixture, set by `ABS_TOL` and `REL_TOL` in the exporter, so loosening it is a diff in all
  16 files. The comparison uses `f64::to_bits`, `is_nan`, and `abs() <=`,
  never `==`, as `clippy::float_cmp` requires.

- **The gate lives in `crates/q-indicators/tests/reference_gate.rs` with a
  `const BINDINGS` table and a sibling `reference_pending.txt`. Pending is a
  bookkeeping state, not an expected failure.** With no kernel, there is
  nothing to run and fail. An "expected to fail" mark would be a mark on
  nothing. Strict bookkeeping gives the same property as pytest's strict
  `xfail`: a bound function still listed as pending fails the gate. Q-022's
  flip for each function is therefore one added `Binding` and one deleted
  line in the same commit, and a reviewer sees both. The pending list stays
  outside `fixtures/reference/` so it is not regenerated and does not show
  up in the staleness diff. The gate lives in `q-indicators` because
  `q-parity` cannot depend on the crate it gates. Later families (Q-025 in
  `q-buffers`, the batch 05 kernels in `q-engine`) each add their own gate
  test and pending file against their own family directory.

- **`q-parity` is a seventh crate, `publish = false`, used only as a
  dev-dependency, with `#![forbid(unsafe_code)]` and the workspace lints.** It
  depends on no `q-*` crate, so every semantic crate can dev-depend on it
  without a cycle. Putting the harness in `q-buffers` would ship JSON fixture
  parsing inside the wheel's dependency graph. Copying it into each crate's
  `tests/` would make the gate exist three times. `parity-isolation` runs
  `cargo tree -e normal,build` for `q-py` and `q-qt`, and fails if `q-parity`
  appears. Cargo's resolver 2 already keeps dev-only features, including
  `float_roundtrip`, out of those builds. The check turns that property into a
  failing build instead of a convention.

- **The loader lists `fixtures/reference/<family>/` and sorts the entries.**
  Listing is the point here: a fixture added without a binding or a pending
  entry must be seen and fail as `Unaccounted`. Invariant 6 forbids
  discovering datasets in production processes. It does not apply to
  committed test data read by a test. Paths come from `env!("CARGO_MANIFEST_DIR")`
  at compile time, inside the repository, so no sibling path and no
  `std::env::var` is involved.

- **The determinism check compares bits, with no tolerance, across two calls
  on separately cloned input vectors. The causality check runs every prefix
  length 1 to n under the fixture's policy.** Determinism means identical
  output, and allowing any tolerance would hide a nondeterministic kernel.
  Separate clones catch a kernel that depends on allocation addresses or
  keeps state between calls. Causality uses the fixture policy, because it
  asks whether the future changes the past, and float-order noise below the
  golden resolution is not information about the future. A full n²
  recomputation over 400 rows and about 30 cases per function is a few million
  element operations, so sampling is unnecessary.

- **Negative controls live in `q-parity`'s own tests, in two groups.** The
  first group runs against the real committed fixtures. The expected outputs
  echoed back pass. At every finite expected value, a perturbation of
  `±2 * max(abs, rel * |e|)` fails and one of `±0.5 * max(abs, rel * |e|)`
  passes. A NaN moved
  by one index fails, and a flipped value bit fails the checksum. The second
  group runs in-memory synthetic fixtures under a `test.` function-id prefix,
  whose kernels are `fn` items in the test module: an identity kernel, an
  identity kernel plus `2e-10` at one index (the synthetic values lie in
  [0, 1], so `rel * |e| <= 1e-12` and the perturbation exceeds both bounds),
  a next-bar kernel whose expected
  output is also next-bar, and an identity kernel that adds a static
  `AtomicU64` call count after its first call. Identity and next-bar kernels
  have no market semantics, so they are not the Rust indicator the spec's
  non-goal rules out. They still exercise every gate path. The counter kernel
  uses an atomic rather than a clock, which clippy disallows. It passes golden
  on the first call and fails determinism on the next two, so the failure is
  attributable to determinism alone.

- **`BACKEND_REV` is a root file beside `CONTRACTS_REV`. The first value is
  `q_backend` `origin/development` `144345acc68ed1586f7e17ea2b2a119cb1195d08`,
  or the `origin/development` head when step 2 runs. The gate compares it with
  every fixture's `backend_rev`.** The commit must be published, because
  `fetch_backend` fetches by hash from GitHub and agents do not push. The
  offline comparison in `cargo test` catches a pin edit without regeneration
  even where `fixtures-check` cannot reach the network. `COMPAT.md` gains a
  "Reference fixture pins" section, not a column, because COMPAT.md is the
  only cross-repository pin record. This pin is test provenance, not a code
  dependency: `q_core` still sits below `q_backend` in §7's direction.

- **`make check` runs `fixtures-test`, `fixtures-check`, and
  `parity-isolation` right after `test`, and before the slow wheel and Qt
  steps.** A stale fixture is the cheapest failure to report, and it is reported
  before minutes of release builds. The GitHub workflow needs no change,
  because it already installs `uv` and runs `make check`. The added wall-clock
  time for the fetch, `uv sync`, and export is measured and reported, not
  assumed.

## Ordered implementation

1. Work on the branch `Q-021-reference-fixtures-and-parity-gate` in `q_core`,
   created from `development` by `./work start`.
2. Add `BACKEND_REV` with the current `q_backend` `origin/development` hash,
   and add `tools/reference/pyproject.toml` pinning `numpy==2.4.4` and
   `pandas==3.0.2` with `uv.lock`. Write failing unittests in
   `test_export_reference.py`. `verify_environment` against a fake checkout
   whose `uv.lock` says `pandas 3.0.1` exits 2 and names pandas. Against a
   lock with the installed versions, it returns the four versions.
   `open_backend_checkout` on a directory whose `HEAD` differs from the rev
   exits 2. Confirm they fail. Implement `fetch_backend`,
   `open_backend_checkout`, `git_blob_id`, and `verify_environment`. Confirm
   they pass, and confirm `fetch_backend` against GitHub with the pinned
   hash yields a checkout containing `tests/backtesting/test_goldens.py`.
   Commit.
3. Write failing unittests. `check_imports` accepts `technical_indicators.py`,
   `moving_averages.py`, `transforms.py`, and `leakage.py` from the fetched
   checkout. It rejects a source containing `import sqlalchemy` or
   `from q_core import compute_rsi`, and it accepts the same line inside
   `if TYPE_CHECKING:`. `load_generator` on the pinned `test_goldens.py`
   returns a callable whose default frame has shape (400, 5) and columns
   `open, high, low, close, volume`. Its `close[0]` equals
   `100.0 + np.random.default_rng(20240609).normal(0.0, 1.0, 400)[0]` exactly.
   A generator source referencing an undefined name raises `NameError`.
   Confirm they fail, implement, and confirm they pass. Commit.
4. Write failing unittests for encoding and provenance. `cpu_feature_level()`
   returns `x86-64-v4` for a fake cpuinfo with `avx512f avx512bw avx512cd
   avx512dq avx512vl` plus the v3 flags, and `x86-64-v3` without the AVX-512
   flags. `compare_trees` on two trees that differ by one ULP in an output
   passes when `cpu_level` differs, fails when it matches, and fails either
   way on a one-ULP change to an input or a changed `backend_rev`.
   `fnv1a64_float64([1.0, -0.0, nan,
   inf])` is `0x892eab94389f5cb0`, and `[]` is `0xcbf29ce484222325`. A NaN
   with bits `0xfff8000000000000` hashes the same as `np.nan`. `encode_column`
   maps NaN to `None`, `inf` to `"inf"`, and `-inf` to `"-inf"`, and keeps
   `-0.0` as `-0.0`. `json.loads` of `dumps_fixture` output returns every
   finite value with identical bits. `dumps_fixture` is byte-identical across
   two calls, ends in a newline, and raises on a raw NaN. Confirm they fail,
   implement, and confirm they pass. Commit.
5. Write failing unittests for cases. `run_case` for `compute_rsi` on
   `monotonic_up_n60` with period 14 gives NaN at indices 0 to 13 and `100.0`
   at 14. On `constant_n60` it gives all NaN. Period 0 gives
   `{"rejected": {"python_exception": "ZeroDivisionError"}}`. `rolling_rank`
   window 0 is rejected with `IndexError`. `donchian_channels` period 0 gives
   all NaN in both outputs. `bollinger_bands` returns outputs named `upper`,
   `middle`, and `lower`. `check_reference_causal` raises the backend's
   `LeakageError` for a spec whose callable is `lambda s: s.shift(-1)`.
   `function_specs()` has exactly the 16 ids. Confirm they fail. Implement
   `build_inputs`, `function_specs`, `run_case`, `check_reference_causal`, and
   `main`. Confirm they pass. Commit.
6. Run `make fixtures`, which is added now, alongside `fixtures-check` and
   `fixtures-test`. Run it a second time into a temp directory and confirm
   `diff -ru` is empty. Record the exporter wall-clock time, the case count
   per function, and the total bytes. Commit the fixtures alone, with the
   `BACKEND_REV` hash in the commit message.
7. Verify the staleness check. `make fixtures-check` passes. Change one value
   in `fixtures/reference/indicators/rsi.json`, confirm it fails and shows that
   line, and revert. Replace `BACKEND_REV` with the previous `q_backend`
   commit, confirm it fails, and revert. Do not commit either change.
8. Create `crates/q-parity` with `publish = false`, the workspace lints,
   `#![forbid(unsafe_code)]`, `serde`, and `serde_json` with
   `float_roundtrip`. Add it to workspace members. Write failing tests in
   `fixture.rs`. `fnv1a64_float64` matches the two literals from step 4.
   `load_reference_set(fixtures/reference, "indicators")` loads 5 inputs and
   16 functions, and every column's checksum verifies. A copy with one value
   changed by one ULP gives `FixtureError::Checksum` naming the column. A
   `format` of `q-core-reference-fixture/2` gives `UnknownFormat`. A case
   referencing `missing.close` gives `DanglingInput`. Confirm they fail,
   implement, and confirm they pass. Commit.
9. Write failing tests in `compare.rs`. Under
   `AbsRelTol { abs: 1e-10, rel: 1e-12 }`, `[1.0]` against `[1.0 + 5e-11]`
   passes through the absolute bound. `[1.0]` against `[1.0 + 2e-10]` exceeds
   both bounds (the relative bound is `1e-12`) and fails with `Magnitude` at
   index 0. `[130000.0]` against `[130000.0 + 1e-8]` is above the absolute
   bound but within the relative bound `1.3e-7`, so it passes only through
   the relative bound. `[130000.0]` against `[130000.0 + 2e-7]` exceeds both
   bounds and fails. `[NaN, 1.0]` against `[1.0, NaN]` fails with
   `NanPlacement` at 0. `[inf]` against `[-inf]` fails with `Infinity`.
   `[-0.0]` against `[0.0]` passes, and lengths 3 against 2 fail with `Length`.
   Under `Exact`, `1.0` against `f64::from_bits(1.0f64.to_bits() + 1)` fails
   with `Bits`, and `-0.0` against `0.0` fails. `compare_outputs` with a
   missing output name fails with `MissingOutput`, and the `Mismatch` display
   contains output, index, expected, and actual. Against the real fixtures,
   every case's expected outputs compared with themselves pass. Confirm they
   fail, implement, and confirm they pass. Commit.
10. Write failing tests in `determinism.rs` and `causality.rs`, using the
    `test.` kernels described in the decisions. The identity kernel passes
    both. The call-counter kernel fails `check_double_run` at output `out`,
    index 0. The next-bar kernel fails `check_prefix_causal` at index 0,
    because `prefix(1)` has no next bar and gives NaN where the full run gives
    `x[1]`, and it names both values. Confirm they fail, implement, and confirm
    they pass. Commit.
11. Write failing tests for the family mechanism. In Rust, a temp fixture root
    with family `test_scenario` holds one file whose cases are
    `{"steps": [{"op": "append", "times": <int64 column>, "close": <float64
    column>}], "expected_after_each": [...]}` under `{"kind": "exact"}`.
    `load_family` returns it with the policy `Exact`. `Column::from_json`
    decodes both columns with verified checksums, and `check_accounting` with
    the id neither bound nor pending gives `Unaccounted`.
    `check_double_run_with` passes a closure that returns a fixed `Vec<i64>`
    and fails a closure over the call counter. In Python, a registered test
    family with `environment = "backend"` makes `main` exit 2 in the minimal
    environment and name the family. `main --family indicators` with
    `--backend-checkout` exits 2 when `detect_environment` reports
    `"backend"`, unless `--allow-numeric-in-backend` is given, the flag human
    criterion 1 uses. Confirm they fail, implement `load_family`,
    `Column::from_json`, `check_accounting`, `check_double_run_with`,
    `Family`, `FAMILIES`, and `detect_environment`, and confirm they pass.
    Add `fixtures-backend` and `fixtures-backend-check` and confirm both
    print the no-op message with `BACKEND_FAMILIES` empty. Commit.
12. Write failing tests in `gate.rs` over an in-memory `ReferenceSet` of `test.`
    fixtures, pinned rev `"a" * 40`:
    - All bound and nothing pending passes.
    - An empty binding table with an empty pending list gives one `Unaccounted`
      per fixture.
    - A function both bound and pending gives `PendingButBound`.
    - A pending id with no fixture gives `PendingUnknown`.
    - A binding with no fixture gives `BoundUnknown`.
    - A fixture whose `backend_rev` is `"b" * 40` gives `ProvenanceRev`.
    - The identity-plus-2e-10 kernel gives `Golden`.
    - The next-bar kernel against next-bar expected outputs gives only
      `NonCausal`.
    - The counter kernel gives only `Nondeterministic`.
    - A kernel returning `Ok` on a `rejected` case gives
      `AcceptedRejectedCase`.
    - A kernel returning `Err` on a computed case gives `UnexpectedError`.
    - `parse_pending` rejects a duplicate id and ignores `#` lines.
    - `summary()` starts with `reference gate: 1 bound, 0 pending, 1 failures`.
    Confirm they fail, implement `run_gate`, and confirm they pass. Commit.
13. Add `q-parity` as a dev-dependency of `q-indicators`, then add
    `reference_pending.txt` with the 16 ids and `reference_gate.rs` with an
    empty `BINDINGS`. Confirm `cargo test -p q-indicators --test
    reference_gate -- --nocapture` prints `reference gate: 0 bound, 16
    pending, 0 failures`. Delete the `rsi` line and confirm it fails with
    `Unaccounted { function_id: "rsi" }`. Add a `vwap` line and confirm
    `PendingUnknown`. Revert both. Commit.
14. Add the `parity-isolation` target and extend `check` in the order given in
    Interfaces. Confirm `make parity-isolation` passes. Add `q-parity` to
    `q-py`'s `[dependencies]`, confirm it fails naming `q-py`, and revert.
    Confirm `cargo test --workspace` passes with networking disabled
    (`unshare -rn cargo test --workspace --offline`). Commit.
15. Update `README.md` with the `q-parity` row, its dev-only edge in the
    dependency direction, and a "Reference fixtures" section. The section
    covers `BACKEND_REV`, `make fixtures`, `make fixtures-check`, what pending
    means, and the Q-022 flip procedure: add a `Binding`, delete the pending
    line, and run the gate. Add the "Reference fixture pins" section to
    `q_contracts/COMPAT.md` on a `Q-021-reference-fixtures-and-parity-gate`
    branch in `q_contracts`. Commit each.
16. Human step, matching human-verifiable criterion 1: create a `q_backend`
    worktree at `BACKEND_REV`, `uv sync --frozen` its full environment, run
    the exporter with `--backend-checkout --family indicators
    --allow-numeric-in-backend` into `/tmp/qb-fixtures`, and
    confirm `diff -ru` against `fixtures/reference` is empty.
17. Human step, matching human-verifiable criterion 2: open `rsi.json` and
    locate the provenance, the `synthetic_ohlcv_n400/period=14` warm-up NaNs
    at indices 0 to 13, the `monotonic_up_n60/period=14` value `100.0`, and
    the `period=0` rejection.
18. Human step, matching human-verifiable criterion 3: push the branch and
    watch the CI run. Record the wall-clock time of the `make check` step
    against the last `development` run.
19. Run the full validation suite and commit. Report the handoff.

## Validation

- **Unit:** exporter environment and checkout verification, import check,
  generator extraction, checksum literals, JSON encoding of NaN, ±inf, and
  -0.0, rejection capture, CPU level detection, and cross-level
  `compare_trees`. Rust checksum literals, loader errors, comparison under
  `AbsRelTol` (including a pass through the absolute bound only, a pass
  through the relative bound only at 130,000, and failures beyond both) and
  `Exact`, double-run, prefix causality, and gate
  bookkeeping.
- **Integration:** the real fixtures load with every checksum verified, compare
  equal to themselves, and pass the indicator gate with 16 pending. The
  exporter regenerates them byte for byte.
- **Regression:** `make check`'s existing steps (fmt, clippy, workspace tests,
  wheel and wheel integration test, Qt harness, contracts drift) still pass.
  `q_backend` has no diff.
- **Manual:** steps 7, 13, and 14 introduce a defect, observe the failure, and
  revert. Steps 16 to 18 are human.
- **Measurement:** exporter wall-clock time, fixture bytes per function and in
  total, cases per function, and the added `make check` time for
  `fixtures-test`, `fixtures-check`, and `parity-isolation`, locally and in
  CI.
- **Pins:** `CONTRACTS_REV` is unchanged. `BACKEND_REV` is new and is recorded
  in `q_contracts/COMPAT.md`.

```bash
cd /home/gui/projects/q/q_core
make fixtures-test
make fixtures-check
make parity-isolation
make fixtures-backend-check                                       # no-op until Q-025 registers bar_window
cargo test -p q-parity
cargo test -p q-indicators --test reference_gate -- --nocapture   # "0 bound, 16 pending, 0 failures"
unshare -rn cargo test --workspace --offline
make check

# staleness negative control (revert afterwards)
sed -i '0,/"period": 14/s//"period": 15/' fixtures/reference/indicators/rsi.json && ! make fixtures-check; git checkout -- fixtures/reference

# human: full-environment equivalence
git -C /home/gui/projects/q/q_backend worktree add /tmp/qb-ref "$(cat BACKEND_REV)"
uv sync --frozen --project /tmp/qb-ref
uv run --frozen --project /tmp/qb-ref python tools/reference/export_reference.py --backend-checkout /tmp/qb-ref --family indicators --allow-numeric-in-backend --out /tmp/qb-fixtures
diff -ru fixtures/reference /tmp/qb-fixtures
```

## Handoff

Report `BACKEND_REV` and confirm it is on `q_backend` `origin/development`.
Report the numpy, pandas, and Python versions and the `cpu_level` recorded in
provenance, and the level of the CI runner if it differs (in which case report
whether `compare_trees` took the policy path and passed). Confirm that all
16 indicator files declare `{"kind": "abs_rel_tol", "abs": 1e-10, "rel":
1e-12}`, and report the two step 9 results that pass through only one bound. For each
of the 16 functions, report the case count and how many cases are rejected.
Report the total fixture bytes and the exporter wall-clock time. Report the
exact `reference gate` summary line from step 13 and the failure lines from
its two negative controls. Report the `make fixtures-check` diff excerpt from
step 7's hand edit and the failure from its pin change. Report the
`parity-isolation` failure from step 14 and confirm that the offline workspace
test passed. List every behavior the exporter captured that a reimplementer
might not expect: RSI on constant input, Donchian period 0, the rolling z-score
standard deviation, the MACD case with fast slower than slow, and HMA period 21.
Q-022 reads that list first. Confirm the `family` mechanism controls from
step 11 passed and name the entry points Q-025 uses: `FAMILIES`,
`load_family`, `check_accounting`, and `check_double_run_with`. From the human steps, report the
full-environment diff result (expected empty), the reviewer's confirmation of
the RSI fixture, the CI run URL, and the added `make check` wall-clock time.
