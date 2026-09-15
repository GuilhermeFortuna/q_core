# Q-022 implementation plan: Indicator and transform kernels

**Status:** authoritative in the [Q project board](https://github.com/users/GuilhermeFortuna/projects/2)  
**Specification:** [`../specs/Q-022-indicator-and-transform-kernels-spec.md`](../specs/Q-022-indicator-and-transform-kernels-spec.md)  
**Depends on:** Q-021

## Current-system context

`crates/q-indicators/src/lib.rs` (18 lines) holds `#![forbid(unsafe_code)]`, a
responsibility paragraph, and `CRATE_NAME`. Its manifest has no dependencies
and inherits the workspace lint table. That table in `Cargo.toml` denies
`float_cmp`, `float_cmp_const`, `imprecise_flops`, and `suboptimal_flops`.
`clippy.toml` disallows `Instant`, `SystemTime`, `env::var`, and `env::var_os`.
A probe crate with the same table confirmed that `suboptimal_flops` fires on
`a * b + c`, which is the shape of every pandas update this task copies, and
that `float_cmp` fires on `a != b`. `crates/q-py` depends on `pyo3 = 0.24`
(`abi3-py312`, `extension-module`; `Cargo.lock` resolves 0.24.2). It exports
`version()` and `contracts_rev()` from a `#[pymodule] fn q_core`, and has no
`numpy` crate. `pyproject.toml` has no runtime dependencies. `tests/test_wheel.py`
installs `dist/*.whl` into a fresh `uv venv` and asserts both callables. `make
check` runs `fmt-check lint test wheel-test qt-test contracts-check`. Q-021
extends it to `fmt-check lint test fixtures-test fixtures-check
parity-isolation wheel-test qt-test contracts-check`. Workspace and `pyproject.toml` versions are both
`2026.9.12`, matching tag `v2026.09.12`. `RELEASING.md` says to tag after `make
check`, but it never says to bump these versions. The skipped bump would make
`q_core.version()` report the previous release. Q-021 provides the 16 pending
fixtures at `fixtures/reference/indicators/<function_id>.json`, with ids
`realized_vol`, `yang_zhang`, `rsi`, `bollinger_bands`, `macd`,
`donchian_channels`, `atr`, `ma_sma`, `ma_ema`, `ma_smma`, `ma_wma`, `ma_hma`,
`rolling_zscore`, `rolling_rank`, `pct_change`, and `clip`. Their shared inputs
are in `fixtures/reference/inputs/`, and each declares `Policy::AbsRelTol {
abs: 1e-10, rel: 1e-12 }`. A finite value passes if `|a - e| <= abs` or `|a -
e| <= rel * |e|`, NaN and infinity positions are exact, and `Policy::Exact`
is bitwise. Provenance records the exporting machine's CPU feature level. Q-021 also provides the test-only crate `q-parity`, with
`publish = false`, which `q-indicators` already carries as a dev-dependency.
The gate `crates/q-indicators/tests/reference_gate.rs` has an empty `const
BINDINGS: &[q_parity::gate::Binding]`, a `PENDING` list included from
`reference_pending.txt` (16 ids), and `BACKEND_REV`. It calls
`q_parity::gate::run_gate(&load_reference_set(root, "indicators"), BINDINGS,
&parse_pending(PENDING), BACKEND_REV)`. `run_gate` performs
`check_accounting`, `compare_outputs`, `check_double_run`, and
`check_prefix_causal` for every case, and a kernel is a `KernelFn = fn(&KernelInputs<'_>,
&Params) -> Result<KernelOutputs, KernelError>`. `make fixtures-check` catches
stale fixtures. `make parity-isolation` fails if `cargo tree -p q-py` or `-p
q-qt` with `-e normal,build` lists `q-parity`.

The reference is `q_backend/src/q_backend/backtesting/technical_indicators.py`
(83 lines), `moving_averages.py` (`compute_ma`, `_wma` at line 47, `_hma` at
line 59), and `transforms.py`, run under pandas 3.0.2 and numpy 2.4.4
(`uv.lock`). I read pandas' `_libs/window/aggregations.pyx` at tag v3.0.2 and
wrote pure-IEEE Python replicas of `roll_mean`, `roll_var`, `_roll_min_max`,
and `ewm`. They are bit-identical to pandas in 154 series-and-window
checks over the golden synthetic close. That covers windows 1 to 50,
including a copy with a NaN, a constant run, an `inf`, and negative values. The
only mismatches were WMA with windows of 16 or more, where `np.dot` goes
through OpenBLAS (`DYNAMIC_ARCH`, Haswell kernel here) and differs from a
sequential sum by at most 7.1e-14. `np.log` and `np.sqrt` matched glibc `log`
and `sqrt` on 200,000 values on this X86_V3 machine. On an AVX-512 machine,
numpy's SIMD `log` can differ from glibc by one ULP. The CPU feature level in
the fixture provenance is what tells a reader whether a realized-volatility or
Yang–Zhang bit difference has that cause. The gap is that none of
these semantics exist in Rust, the 16 fixtures are pending, and Python cannot
reach any kernel.

## Interfaces produced

```rust
// crates/q-indicators/src/error.rs
#[derive(Debug, Clone, PartialEq)]
pub enum IndicatorError {
    /// A parameter outside the domain the Python reference accepts.
    InvalidParameter { function: &'static str, parameter: &'static str, value: i64, requirement: &'static str },
    /// clip: low > high (both non-NaN).
    InvalidBounds { low: f64, high: f64 },
    /// Price columns that must align have different lengths.
    LengthMismatch { function: &'static str, parameter: &'static str, expected: usize, actual: usize },
}
impl core::fmt::Display for IndicatorError { /* "rsi: period must be >= 1 (got 0)" */ }
impl std::error::Error for IndicatorError {}
```

```rust
// crates/q-indicators/src/ieee.rs            (pub(crate); the only audited float equality)
pub(crate) fn ieee_eq(a: f64, b: f64) -> bool;          // C `==`; #[expect(clippy::float_cmp, reason = ...)]
pub(crate) fn window_missing(x: f64) -> bool;           // NaN or ±inf: pandas _prep_values
pub(crate) fn com_from_span(span: i64) -> f64;          // (span - 1) / 2
pub(crate) fn com_from_alpha_period(period: i64) -> f64; // alpha = 1/period; (1 - alpha) / alpha

// crates/q-indicators/src/window.rs          (pub(crate) pandas window primitives)
pub(crate) fn rolling_mean(values: &[f64], window: usize) -> Vec<f64>;   // roll_mean, min_periods = window
pub(crate) fn rolling_var(values: &[f64], window: usize) -> Vec<f64>;    // roll_var, ddof = 1
pub(crate) fn rolling_std(values: &[f64], window: usize) -> Vec<f64>;    // zsqrt(rolling_var)
pub(crate) fn rolling_max(values: &[f64], window: usize) -> Vec<f64>;    // _roll_min_max is_max = 1
pub(crate) fn rolling_min(values: &[f64], window: usize) -> Vec<f64>;
pub(crate) fn rolling_linear_wma(values: &[f64], window: usize) -> Vec<f64>; // _wma via roll_apply
pub(crate) fn ewm_mean(values: &[f64], com: f64, min_periods: usize) -> Vec<f64>; // adjust=False, ignore_na=False

// crates/q-indicators/src/elementwise.rs
pub(crate) fn shift(values: &[f64], lag: usize) -> Vec<f64>;     // NaN-filled head
pub(crate) fn diff(values: &[f64]) -> Vec<f64>;                  // x[i] - x[i-1]
pub(crate) fn clip_lower(values: &[f64], low: f64) -> Vec<f64>;  // Series.clip(lower=)
```

```rust
// crates/q-indicators/src/indicators.rs
pub fn realized_vol(close: &[f64], window: i64, periods_per_year: i64) -> Result<Vec<f64>, IndicatorError>;
pub fn yang_zhang(open: &[f64], high: &[f64], low: &[f64], close: &[f64], window: i64, periods_per_year: i64) -> Result<Vec<f64>, IndicatorError>;
pub fn rsi(close: &[f64], period: i64) -> Result<Vec<f64>, IndicatorError>;
pub struct BollingerBands { pub upper: Vec<f64>, pub middle: Vec<f64>, pub lower: Vec<f64> }
pub fn bollinger_bands(close: &[f64], period: i64, num_std: f64) -> Result<BollingerBands, IndicatorError>;
pub struct Macd { pub line: Vec<f64>, pub signal: Vec<f64>, pub histogram: Vec<f64> }
pub fn macd(close: &[f64], fast_period: i64, slow_period: i64, signal_period: i64) -> Result<Macd, IndicatorError>;
pub struct DonchianChannels { pub upper: Vec<f64>, pub lower: Vec<f64> }  // upper.len() == high.len(), lower.len() == low.len()
pub fn donchian_channels(high: &[f64], low: &[f64], period: i64) -> Result<DonchianChannels, IndicatorError>;
pub fn atr(high: &[f64], low: &[f64], close: &[f64], period: i64) -> Result<Vec<f64>, IndicatorError>;

// crates/q-indicators/src/moving_averages.rs
pub fn sma(values: &[f64], period: i64) -> Result<Vec<f64>, IndicatorError>;
pub fn ema(values: &[f64], period: i64) -> Result<Vec<f64>, IndicatorError>;
pub fn smma(values: &[f64], period: i64) -> Result<Vec<f64>, IndicatorError>;
pub fn wma(values: &[f64], period: i64) -> Result<Vec<f64>, IndicatorError>;
pub fn hma(values: &[f64], period: i64) -> Result<Vec<f64>, IndicatorError>;

// crates/q-indicators/src/transforms.rs
pub fn rolling_zscore(values: &[f64], window: i64) -> Result<Vec<f64>, IndicatorError>;
pub fn rolling_rank(values: &[f64], window: i64) -> Result<Vec<f64>, IndicatorError>;
pub fn pct_change(values: &[f64], change_bars: i64) -> Result<Vec<f64>, IndicatorError>;
pub fn clip(values: &[f64], low: f64, high: f64) -> Result<Vec<f64>, IndicatorError>;

// crates/q-indicators/src/lib.rs
#![forbid(unsafe_code)]
pub use error::IndicatorError;
pub use indicators::*; pub use moving_averages::*; pub use transforms::*;
pub const CRATE_NAME: &str = "q-indicators";   // doc comment no longer says "only public item"
```

```rust
// crates/q-py/src/indicators.rs              (projection only)
// Submodule `q_core.indicators`, also inserted into sys.modules so `import q_core.indicators` works.
#[pyfunction] fn rsi<'py>(py: Python<'py>, close: PyReadonlyArray1<'py, f64>, period: i64) -> PyResult<Bound<'py, PyArray1<f64>>>;
#[pyfunction] fn bollinger_bands<'py>(py: Python<'py>, close: PyReadonlyArray1<'py, f64>, period: i64, num_std: f64)
    -> PyResult<(Bound<'py, PyArray1<f64>>, Bound<'py, PyArray1<f64>>, Bound<'py, PyArray1<f64>>)>;  // (upper, middle, lower)
// ... and realized_vol(close, window, periods_per_year=252), yang_zhang(open, high, low, close, window, periods_per_year=252),
//     macd(close, fast_period, slow_period, signal_period) -> (line, signal, histogram),
//     donchian_channels(high, low, period) -> (upper, lower), atr(high, low, close, period),
//     sma/ema/smma/wma/hma(values, period), rolling_zscore(values, window), rolling_rank(values, window),
//     pct_change(values, change_bars), clip(values, low, high)
fn contiguous<'a>(argument: &'static str, array: &'a PyReadonlyArray1<'_, f64>) -> PyResult<&'a [f64]>; // ValueError if not C-contiguous
fn indicator_error(err: IndicatorError) -> PyErr;                                                      // ValueError(err.to_string())
fn register(parent: &Bound<'_, PyModule>) -> PyResult<()>;

// crates/q-py/Cargo.toml
numpy = "0.24"                                     // same entry Q-025 adds; whichever lands second reuses it
// pyproject.toml
[project] dependencies = ["numpy>=2"]
```

```rust
// crates/q-indicators/tests/reference_gate.rs   (Q-021's file; Q-022 fills BINDINGS and adds adapters and a report test)
const BINDINGS: &[q_parity::gate::Binding] = &[
    q_parity::gate::Binding { function_id: "rsi", kernel: bind_rsi },
    // ... one entry per id in Current-system context, e.g. "ma_hma" -> bind_ma_hma
];
fn bind_rsi(inputs: &q_parity::gate::KernelInputs<'_>, params: &q_parity::fixture::Params)
    -> Result<q_parity::gate::KernelOutputs, q_parity::gate::KernelError>;   // and 15 more bind_<function_id>
fn param_i64(params: &q_parity::fixture::Params, name: &str) -> Result<i64, q_parity::gate::KernelError>;  // ParamValue::Int
fn param_f64(params: &q_parity::fixture::Params, name: &str) -> Result<f64, q_parity::gate::KernelError>;  // ParamValue::Float (or Int, converted)
fn rejection(err: q_indicators::IndicatorError) -> q_parity::gate::KernelError;   // KernelError { message: err.to_string() }
#[test] fn indicator_reference_gate();          // Q-021's; unchanged
#[test] fn indicator_bit_identity_report();     // Q-022: per-function bit-identical / within-tolerance counts

// crates/q-indicators/tests/reference_pending.txt   16 ids -> empty (comments only)
```

```
clippy.toml                                   disallowed-methods += f64::mul_add, f32::mul_add
tests/test_wheel.py                           extended: indicator projection checks
tools/bench/compare_pandas.py                 human benchmark; imports q_backend and q_core.indicators, prints runs and medians
RELEASING.md                                  version-bump step added
Cargo.toml, pyproject.toml                    version -> release date
```

## Implementation decisions

- **The kernels copy pandas' algorithms, not their definitions.** `rolling_mean`
  is `roll_mean`: Kahan-compensated add and remove with separate
  `compensation_add` and `compensation_remove`, a `neg_ct` that clamps a
  wrong-signed mean to 0, and `num_consecutive_same_value >= nobs` returning
  `prev_value`. `rolling_var` is 3.0.2's `roll_var`: Welford with Kahan,
  `nobs` held as f64, a `numerically_unstable` flag set when `prev_m2 *
  InvCondTol > ssqdm_x` (with `InvCondTol = f64::EPSILON * 1e3`) that forces a
  recompute of the window, and no same-value shortcut. Removes run before adds.
  Recomputing each window from scratch is simpler, but it differs from pandas in
  the last bits after every window slide. The replica showed that copying the
  update order gives bit identity, so the tolerance is kept for real platform
  differences instead of spent on our own drift. Both are still causal and
  deterministic, because the state is a function of the prefix.

- **Window bounds are pandas' `FixedWindowIndexer`: `start = max(0, i + 1 - w)`,
  `end = i + 1`. Rolling `min_periods` is the window, and `roll_var` and
  `_roll_min_max` raise it to at least 1.** A window of 0 therefore yields empty
  windows and NaN everywhere. That is what the reference returns for Bollinger,
  Donchian, z-score, realized volatility, and Yang–Zhang at 0. The Cython reads
  `values[s]` one past the end at window 0 without a bounds check. The Rust
  kernel guards that read, and the value is unused because `nobs` is 0.

- **Every window and EWM primitive first maps ±inf to NaN, and elementwise steps
  do not.** `BaseWindow._prep_values` converts infinities before every Cython
  call. RSI on a close containing `inf` gives `[nan, nan, nan, 100, 100, 100]`
  only because `diff` produces `inf` and `-inf`, which the EWM then treats as
  missing. Rolling rank is a Python loop that tests `np.isnan` only, so `inf` is
  a value there (`[nan, 1.0, 0.5, 1.0]` for `[1, inf, 2, inf]`, window 2).

- **`ewm_mean` is `aggregations.ewm` with `adjust=False` and `ignore_na=False`,
  including the `com == 1` branch.** `alpha = 1 / (1 + com)` is computed from the
  center of mass, not from span or period directly, because pandas round-trips
  through `get_center_of_mass`. On a missing input, `old_wt *= 1 - alpha` and
  the output carries the previous value. `old_wt` resets to 1 only on an
  observation. When `com == 1` (span 3, or period 2 for SMMA, RSI, and ATR),
  pandas sets `new_wt = 1 - old_wt`. With `[1, NaN, 3, 4]`, this gives SMMA
  period 2 `[1, 1, 2.5, 3.25]`, while EMA span 4 gives `[1, 1,
  2.0526315789473686, 2.8315789473684214]`. The `weighted != cur` skip is kept
  through `ieee_eq`. `min_periods` is `max(min_periods, 1)`. RSI and ATR pass
  `period`, and EMA, SMMA, and MACD pass 0.

- **`rolling_max` and `rolling_min` use the monotonic deque with pandas' tie rule
  (`>=` pops for max, `<=` for min).** The rule only matters for which of 0.0
  and -0.0 is returned, but copying it costs nothing and makes these kernels
  exactly equal. A window containing any missing value is NaN.

- **`rolling_linear_wma` sums `x[k] * (k + 1)` sequentially and divides by
  `(w * (w + 1) / 2) as f64`. A window is NaN unless `i >= w - 1` and all `w`
  values are finite.** `roll_apply` computes only when
  `roll_sum(isfinite(arr)) >= minp`. `weights.sum()` is an exact integer.
  NumPy's `np.dot` is OpenBLAS: sequential below 16 elements, and a CPU-specific
  AVX2 or AVX-512 accumulation above that. Copying one CPU's kernel would make
  the result depend on the CPU. The measured gap of 7.1e-14 at price level 100
  is a relative error of about 7e-16. `AbsRelTol`'s `rel = 1e-12` covers that
  gap at any scale, including WIN$N at about 130,000 points, where the absolute
  1e-10 alone would be marginal. The informational report counts it (spec
  criterion 2).

- **HMA is `half = max(period / 2, 1)` (integer division), `sqrt_p =
  max((period as f64).sqrt() as i64, 1)`, `raw = 2.0 * wma(half) - wma(period)`,
  `hma = wma(raw, sqrt_p)`.** `int(np.sqrt(period))` truncates a correctly
  rounded square root, and Rust's `sqrt` is correctly rounded too, so the two
  agree for every period below 2^52. The first valid index is `(period - 1) +
  (sqrt_p - 1)`: 10 for period 9, and 4 for period 4 on `arange(10)`, which
  gives `[nan×4, 4, 5, 6, 7, 8, 9]`.

- **The composite functions keep the reference's operation order and
  association:**
  - Realized volatility: `ln(close / shift(close, 1))`, then `rolling_std`,
    then `* (ppy as f64).sqrt()`.
  - Yang–Zhang: `u * (u - c) + d * (d - c)`, `k = 0.34 / (1.34 + (w + 1) as f64
    / (w - 1) as f64)`, and `(var_on + k * var_oc) + (1.0 - k) * mean_rs`. Its
    final `sqrt` is plain, so a negative variance gives NaN, while rolling std
    uses `zsqrt`, so a negative gives 0.
  - RSI: `100.0 - (100.0 / (1.0 + rs))`, with gains and losses from
    `clip_lower(diff, 0.0)`, which keeps NaN and -0.0.
  - ATR: true range is the NaN-skipping maximum of the three terms, and NaN
    only when all three are NaN. This is `DataFrame.max(axis=1)`, so bar 0 is
    `high - low`.
  - Donchian: `shift(rolling_max(high, p), 1)`.
  - Z-score: `(x - mean) / std` on the unprepared `x`, so a constant window
    gives `0 / 0 = NaN`.
  - Percent change: `x / shift(x, lag) - 1.0`.
  - Clip: `x >= low ? x : low`, then `r <= high ? r : high`. NaN stays NaN, and
    a NaN bound skips that side.

  Rust never contracts floating-point expressions and does not reorder sums
  without fast-math. The source order is therefore the evaluation order, and
  that is the order the Cython and numpy code used.

- **`#[expect(clippy::suboptimal_flops, reason = "pandas evaluates a*b+c
  unfused; mul_add changes the result bits")]` goes on each function that needs
  it. `f64::mul_add` and `f32::mul_add` are added to `disallowed-methods`.** The
  workspace lint would otherwise push the code toward `mul_add`, which is fused
  and gives different bits from pandas. `expect` fails once the lint no longer
  fires, so stale allowances cannot pile up, and the disallowed method turns
  "never fuse" into an enforced rule (spec criterion 6). Float equality
  appears once, in `ieee_eq`, under `#[expect(clippy::float_cmp)]`, so that
  every exact comparison is a call to one audited helper.

- **Window and period parameters are `i64` in the public API, and each function
  validates first, exactly where the reference raises.**
  - `rsi` and `atr`: period `< 1`. The reference raises `ZeroDivisionError` at
    0 and `alpha must satisfy` below 0.
  - `yang_zhang`: window `< 0` or `== 1`. The reference raises
    `ZeroDivisionError` at 1, and window 0 returns NaN.
  - `realized_vol`, `bollinger_bands`, `donchian_channels`, and
    `rolling_zscore`: window `< 0`.
  - `sma`, `ema`, `smma`, `wma`, and `hma`: period `< 1`.
  - `macd`: any span `< 1`.
  - `rolling_rank`: window `< 1` when `values` is non-empty. The reference
    raises `IndexError`, and an empty input returns empty even with window 0.
  - `pct_change`: `change_bars < 1`.
  - `clip`: `low > high`.

  Q-021's gate fails a kernel that accepts what the reference rejects. With
  `usize`, a negative window could not reach the kernel, and the rejection would
  move into test adapters that nothing else runs. Validation comes before the
  empty-input shortcut in every function except `rolling_rank`, because that is
  the reference's order. `periods_per_year` is `i64` converted with `as f64`,
  which is exact below 2^53, and a negative value yields NaN outputs, as
  `np.sqrt` does.

- **`IndicatorError` surfaces in Python as `ValueError(str(err))`, whatever
  exception type the reference raised.** For the inputs where the reference
  raised `ZeroDivisionError` (RSI and ATR at period 0, Yang–Zhang at window 1)
  or `IndexError` (rolling rank at window 0 or less), this is the one accepted
  behavior change. Q-023 keeps parameter validation in the backend with today's
  messages, so a backend caller normally never reaches a `q_core`
  rejection. The error is the kernel's own contract and the
  gate's rejection signal, not a reproduction of Python's exception types.
  Mapping `ZeroDivisionError` per parameter would copy accidents into a new API.

- **Yang–Zhang and ATR reject unequal column lengths. Donchian does not, and each
  output has its own input's length.** The reference computes Donchian upper
  and lower from `high` and `low` independently, while Yang–Zhang and ATR
  combine columns by index alignment. Arrays have no index, so pairing columns
  of different lengths can only be a caller bug.

- **q-py uses the `numpy` crate 0.24, which pairs with pyo3 0.24, rather than
  pyo3's buffer protocol. It is the same dependency entry Q-025 adds.**
  `pyo3::buffer` is available under `abi3-py312` (`src/buffer.rs` is
  `cfg(any(not(Py_LIMITED_API), Py_3_11))`), so both options are possible. With
  the buffer protocol, float64 must be checked through format strings (`d`,
  `<d`, `=d`), and outputs have no zero-copy route: they need a Python-level
  `numpy.empty` plus a copy, or `frombuffer` over a second allocation.
  `PyReadonlyArray1<f64>` refuses a wrong dtype with `TypeError` at extraction,
  and its `as_slice` refuses non-contiguous memory. `PyArray1::from_vec` hands
  the kernel's `Vec` to numpy without copying. Its shared borrow flags also stop
  another Rust extension from holding a mutable view of the same array. rust-numpy
  talks to NumPy only through the `_ARRAY_API` capsule and its own struct
  definitions, and its FFI layer avoids non-limited CPython calls. Step 2
  verifies abi3 compatibility before any kernel depends on it. If the abi3
  build or import fails, the fallback is the buffer protocol with a
  `numpy.empty` output filled through `as_mut_slice`, and the choice is
  recorded in the handoff.

- **Inputs are never cast or copied, and the GIL is held during computation.** A
  silent cast would hide an O(n) copy and a dtype bug from the caller, and Q-023
  passes `Series.to_numpy(dtype=np.float64)`, which is already contiguous. The
  kernels read memory borrowed from a numpy array. Only the GIL stops Python
  code in another thread from writing that memory during the call. Releasing it
  would give a Rust `&[f64]` aliasing live writes. Kernel cost is measured
  (human step), and releasing the GIL can be revisited with a copying variant if
  worker threads show contention.

- **Python surface is the submodule `q_core.indicators`, with one function per
  fixture function and the reference's argument order and defaults
  (`periods_per_year=252`). The moving averages are five functions, not
  `compute_ma(type)`.** One function per fixture keeps the gate, the Rust API,
  and the wheel in one-to-one correspondence. MA-type normalization is Python
  string handling that Q-023 keeps in `normalize_ma_type`. The submodule leaves
  room for later kernel families without flattening them into the top-level
  namespace, and `version()` and `contracts_rev()` stay at the top level.

- **The window primitives are `pub(crate)`.** The Q-021 fixtures judge only the
  16 public functions. A public `rolling_var` would be semantics with no gate
  over it, and Q-026 and later tasks can promote one when they bring a fixture
  for it.

- **Kernels are bound in Q-021's `crates/q-indicators/tests/reference_gate.rs`
  as `q_parity::gate::Binding` entries in `BINDINGS`, one `bind_<function_id>`
  adapter each, and `q-parity` stays a dev-dependency of `q-indicators` only.**
  An adapter reads its columns with `KernelInputs::column` under the Python
  argument names the fixture's `Case.inputs` records (`close`, `high`, ...),
  and its parameters from `Params` under the Python keyword names
  (`period`, `window`, `num_std`, `change_bars`, ...). It calls the public
  kernel and returns `KernelOutputs` keyed by the output names in the fixture's
  `Expected::Outputs` (Q-021 names Bollinger `upper`, `middle`, `lower`). It
  maps `IndicatorError` to `KernelError`. The adapter holds no semantics, so
  `run_gate` judges the public function itself. A kernel error on a
  `Rejected` case satisfies the gate, and an error elsewhere is
  `UnexpectedError`. `run_gate` already runs `check_accounting`,
  `check_double_run`, and `check_prefix_causal`. This task does not call
  `load_family`, `Column::from_json`, or `check_double_run_with`, which exist
  for scenario families such as Q-025's. Adding `q-indicators` to q-py's normal
  dependencies does not pull in `q-parity`, because Cargo does not propagate a
  dependency's dev-dependencies. `make parity-isolation` therefore keeps
  passing. Steps 2 and 13 run it, because those are the steps that change
  q-py's graph.

- **The release bumps `[workspace.package] version` and `pyproject.toml`
  `version` to the release date (for example `2026.9.20` for tag
  `v2026.09.20`), and `RELEASING.md` gains that step.** `version()` reads
  `CARGO_PKG_VERSION`, and `tests/test_wheel.py` asserts it equals the
  workspace version. Tagging without the bump publishes a wheel that reports the
  previous release. The bump is the last commit on the task branch. The human
  who merges and tags adjusts the date if the merge happens on another day,
  because agents neither merge nor push.

- **`pyproject.toml` declares `numpy>=2` as a runtime dependency.** The
  projection imports numpy's C API at module load, so a wheel without the
  dependency imports into an environment where the first call fails.
  `q_backend` locks numpy 2.4.4, and nothing tests 1.x.

## Ordered implementation

- [x] 1. Work on the branch `Q-022-indicator-and-transform-kernels` in `q_core`,
   created from `development` by `./work start`.
- [x] 2. Projection spike. Add `numpy = "0.24"` to `crates/q-py/Cargo.toml`, or reuse
   the entry if Q-025 has already landed. Add `numpy>=2` to `pyproject.toml`.
   Add a temporary `q_core.indicators.identity(values)` that returns
   `PyArray1::from_vec` of a copy. Extend `tests/test_wheel.py` to install numpy
   and assert that a float64 array round-trips and that an `int64` array raises
   `TypeError`. Confirm the test fails before the binding exists. Run `make
   wheel-test` and confirm the wheel tag is still `cp312-abi3` and the test
   passes. Run `make parity-isolation` and confirm `q-parity` is still absent
   from q-py's graph. If the abi3 build or import fails, switch to the buffer-protocol
   fallback here and record why. Remove `identity` in step 13. Commit.
- [x] 3. Add `IndicatorError` and `ieee.rs`. Write failing tests:
   `com_from_span(3) == 1.0`; `com_from_alpha_period(2)` has the bits of
   Python's `(1 - 0.5) / 0.5`; `com_from_alpha_period(14)` has the bits of
   `(1 - 1/14) / (1/14)` (literal taken from Python);
   `window_missing(f64::INFINITY)`; `!window_missing(0.0)`; and `Display` of an
   `InvalidParameter` reads `rsi: period must be >= 1 (got 0)`. Add the
   `mul_add` disallowed methods to `clippy.toml`. Confirm the tests fail,
   implement, and confirm they pass. Commit.
- [x] 4. Write failing tests for `rolling_mean` and `rolling_var`/`rolling_std`, with
   values produced by pandas 3.0.2:
   - Mean, window 2 over `[0.1, 0.2, 0.3, 0.1, 0.1]`: `[nan,
     0.15000000000000002, 0.25, 0.2, 0.1]`.
   - Mean, window 3 over `[1e16, 1, 1, 1]`: `[nan, nan, 3333333333333334.0,
     1.0]`. The last value is the same-value shortcut.
   - Mean, window 2 over `[1e16, 1, 2, -3]`: `[nan, 5e15, 1.5, -0.5]`.
   - Var, window 2 over `[1e15, 1, 2, 4]`: `[nan, 4.99999999999999e29, 0.5,
     2.0]`. This is the instability recompute.
   - Var, window 3 over `[1e15, 1, 2, 4, 8]`: `[nan, nan, 3.333333333333323e29,
     2.333333333333333, 9.333333333333332]`.
   - Std, window 2 over `[3, 3, 3]`: `[nan, 0, 0]`.
   - Std, window 3 over `[0.1, 0.2, 0.3, 0.3, 0.3, 0.3]`: `[nan, nan,
     0.09999999999999998, 0.05773502691896253, 0, 0]`.
   - Window 0 gives all NaN, an `inf` input behaves as NaN, and an empty input
     gives an empty output.

   Compare bits with `to_bits`, treating NaN positions separately. Confirm they
   fail, implement, and confirm they pass. Commit.
- [x] 5. Write failing tests for `rolling_max`, `rolling_min`, `shift`, `diff`, and
   `clip_lower`: max window 2 over `[1, NaN, 3, 2]` is `[nan, nan, nan, 3]`;
   Donchian's composition over `high = [1, 3, 2, 5, 4]` and `low = [0, 1, 1, 2,
   3]` with period 2 gives upper `[nan, nan, 3, 3, 5]` and lower `[nan, nan, 0,
   1, 1]`; `clip_lower(-0.0, 0.0)` keeps -0.0. Confirm they fail, implement,
   and confirm they pass. Commit.
- [x] 6. Write failing tests for `ewm_mean`: SMMA (`com_from_alpha_period(2)`,
   `min_periods` 0) over `[1, NaN, 3, 4]` is `[1, 1, 2.5, 3.25]`; EMA span 4 is
   `[1, 1, 2.0526315789473686, 2.8315789473684214]`; span 3 equals the SMMA
   period-2 result; leading NaNs stay NaN until the first observation; an
   `inf` is carried like NaN. Confirm they fail, implement, and confirm they
   pass. Commit.
- [x] 7. Write failing tests for `rolling_linear_wma`: window 2 over `[1, 2, inf, 3,
   4]` is `[nan, 1.6666666666666667, nan, nan, 3.6666666666666665]`; window 1
   is the identity; a 40-element window-20 case matches the sequential literal
   computed in Python. Confirm they fail, implement, and confirm they pass.
   Commit.
- [x] 8. Bind and implement the moving averages. Bind `sma`, `ema`, `smma`, `wma`, and
   `hma` as `ma_sma`, `ma_ema`, `ma_smma`, `ma_wma`, and `ma_hma` in
   `BINDINGS` in `reference_gate.rs`. Their `bind_ma_*` adapters call kernels
   that return `Ok(vec![f64::NAN; n])`. In the same change, delete those five
   lines from `crates/q-indicators/tests/reference_pending.txt`, because leaving
   one produces `GateFailure::PendingButBound`. Run `cargo test -p q-indicators
   --test reference_gate` and confirm it fails with `GateFailure::Golden`,
   naming the function, case, output, and first index. Add unit
   tests: `hma(arange(10), 4)` is `[nan×4, 4, 5, 6, 7, 8, 9]`; `hma` over
   `arange(30)` with period 9 has its first valid value at index 10;
   `hma(x, 1)` equals `x`; period 0 gives `InvalidParameter` for all five.
   Implement them. Confirm the unit tests and the gate pass for these five,
   including determinism and causality. Commit.
- [x] 9. Bind and implement the transforms the same way: add `rolling_zscore`,
   `rolling_rank`, `pct_change`, and `clip` stubs to `BINDINGS`, delete their
   lines from `reference_pending.txt`, and confirm the gate fails. Unit tests:
   - `rolling_rank([1, inf, 2, inf], 2)` is `[nan, 1.0, 0.5, 1.0]`.
   - `rolling_rank([], 0)` is `Ok([])`.
   - `rolling_rank([1.0], 0)` and `rolling_rank([1.0], -1)` are errors.
   - `rolling_zscore([2; 5], 3)` is all NaN.
   - `rolling_zscore(x, 0)` is all NaN, and window `-1` is an error.
   - `pct_change([0, 1, 0, 0], 1)` is `[nan, inf, -1, nan]`, and lag 0 is an
     error.
   - `clip([NaN, -5, 5, 0.5], 0, 1)` is `[nan, 0, 1, 0.5]`.
   - `clip([1, 2, 4, 3, 5], NaN, 3)` is `[1, 2, 3, 3, 3]`.
   - `clip(x, 3, 2)` is `InvalidBounds`.

   Implement them, and confirm the tests and the gate pass. Commit.
- [x] 10. Bind and implement the indicators the same way: add stubs for
    `realized_vol`, `yang_zhang`, `rsi`, `bollinger_bands`, `macd`,
    `donchian_channels`, and `atr` to `BINDINGS`, delete their lines from
    `reference_pending.txt` (leaving it with no ids), and confirm the gate
    fails. Unit tests:
    - `rsi([1, 2, 3, 4, 5], 2)` is `[nan, nan, 100, 100, 100]`, and a flat
      series is all NaN.
    - `rsi([1, 2, 4, 3, 5], 1)` is `[nan, 100, 100, 0, 100]`.
    - `rsi(x, 0)` and `rsi(x, -2)` are errors.
    - `atr(high = S + 1, low = S - 1, close = S, 1)` with `S = [1, 2, 4, 3, 5]`
      is `[2, 2, 3, 2, 3]`.
    - `atr` over `[nan, 2, 3, 4]` / `[nan, 1, 2, 3]` / `[nan, 1.5, 2.5, 3.5]`
      with period 2 is `[nan, nan, 1.25, 1.375]`.
    - `macd` over `[nan, nan, 1, 2]` with spans (2, 3, 2) matches pandas'
      `[nan, nan, 0, 0.16666666666666652]` line.
    - `macd` with spans (1, 1, 1) gives zeros, and a span of 0 is an error.
    - `yang_zhang` with window 1 is an error, window 0 is all NaN, and window
      `-1` is an error.
    - `realized_vol([1, 2, 4, 3, 5], 2, 0)` is `[nan, nan, 0, 0, 0]`.
    - `bollinger_bands` with period 1 gives `middle == close` and NaN bands.
    - `donchian_channels` with period 0 is all NaN, and period `-1` is an
      error.
    - `yang_zhang` and `atr` with a short `low` give `LengthMismatch`.

    Implement them, and confirm the tests and the gate pass. Commit.
- [x] 11. Confirm that `GateReport::summary()` reads `reference gate: 16 bound, 0
    pending, 0 failures`. Add `indicator_bit_identity_report` to
    `reference_gate.rs`. It loads the set with `q_parity::fixture::load_reference_set`,
    resolves each `Case.inputs` `ColumnRef` against `ReferenceSet.inputs`, runs
    the bound kernel, and counts per function the finite expected values whose
    bits equal the output (`to_bits`, NaN canonicalised) and those that pass
    only the fixture's `Policy::AbsRelTol`. It prints the counts and the largest
    relative difference, and it asserts nothing: the report is informational,
    and `indicator_reference_gate` is the gate. If a function other than WMA and
    HMA is not fully bit-identical, trace the divergent operation against the
    pandas source. For realized volatility and Yang–Zhang, check the CPU feature
    level in the fixture provenance for the AVX-512 `log` cause. Record the
    cause for the handoff either way. Commit.
- [x] 12. Negative control for spec criterion 6: add a `mul_add` call in `window.rs`,
    run `make lint`, and confirm it fails naming the disallowed method. Revert,
    and do not commit.
- [x] 13. Projection. Extend `tests/test_wheel.py` first, with these assertions:
    - Each of the 16 names exists on `q_core.indicators`, and `import
      q_core.indicators` works.
    - `pct_change(np.array([0., 1., 0., 0.]), 1)` equals `[nan, inf, -1, nan]`.
    - `sma(np.array([1., 2., 3.]), 2)` equals `[nan, 1.5, 2.5]`.
    - `rsi(np.arange(1., 20.), 14)[-1] == 100.0`.
    - `bollinger_bands` returns a 3-tuple and `donchian_channels` a 2-tuple.
    - `rsi(x, 0)` raises `ValueError` with `period` in the message.
    - An `int64` input raises `TypeError`, and `x[::2]` raises `ValueError`.
    - The input is equal to a copy taken before the call.
    - `np.shares_memory(out, x)` is false.
    - `version()` and `contracts_rev()` are unchanged.

    Confirm the test fails. Implement `crates/q-py/src/indicators.rs`, register
    the submodule and its `sys.modules` entry, and remove the spike's `identity`.
    Run `make wheel-test` and `make parity-isolation`, and confirm both pass. Commit.
- [x] 14. Add `tools/bench/compare_pandas.py`. It generates a fixed-seed close, OHLC
    of one million bars, and 100,000 bars for rolling rank. For RSI(14),
    Bollinger(20, 2), WMA(20), HMA(20), Yang–Zhang(20), and rolling rank(20),
    it times `q_backend`'s pandas function and the `q_core.indicators` function
    over five runs each with `time.perf_counter_ns`. It prints each run and the
    median, and first asserts that the two results agree under the fixture
    tolerance. Run it once on 10,000 bars to confirm it executes. Commit.
- [x] 15. Release preparation. Add the version-bump step to `RELEASING.md`, bump
    `Cargo.toml` `[workspace.package] version` and `pyproject.toml` `version` to
    today's date, refresh `Cargo.lock`, update the `CRATE_NAME` doc comment and
    the `q-indicators` row in `README.md` to name the 16 functions, and run
    `make wheel-test`. Commit.
- [ ] 16. Human step, matching human-verifiable criterion 3: watch the branch's CI run.
- [ ] 17. Human step, matching human-verifiable criterion 2: run
    `tools/bench/compare_pandas.py` against the built wheel in the `q_backend`
    environment, and record per-function runs and medians.
- [ ] 18. Human step, matching human-verifiable criterion 1: after merging, adjust the
    version date if needed, tag `vYYYY.MM.DD`, push the tag, and resolve and
    call the wheel from the tag in a clean environment.
- [x] 19. Run the full validation suite, `make check`, and confirm it passes. Commit
    any fixes.

## Validation

- **Unit:** each window primitive against pandas 3.0.2 literals, compared by
  bits; EWM's NaN carry and `com == 1` branch; infinity handling in windows
  compared with rolling rank; every degenerate parameter's result or rejection;
  length mismatches; error messages.
- **Integration:** `indicator_reference_gate` with all 16 ids in `BINDINGS` and
  `reference_pending.txt` empty (`run_gate`: accounting, goldens under
  `Policy::AbsRelTol`, rejections, `check_double_run`, `check_prefix_causal`); the wheel installed
  into a clean venv and called through numpy.
- **Regression:** the gate itself is the locked reference. `version()` and
  `contracts_rev()` are unchanged, `make fixtures-check` still passes,
  so fixtures under `fixtures/reference/` are untouched, and `make
  parity-isolation` passes with `numpy` and the indicators in q-py's graph.
- **Manual:** steps 16 and 18.
- **Measurement:** step 11's bit-identity counts per function; step 17's pandas
  compared with `q_core` timings, five runs each, individual values and medians.
- **Pins:** `CONTRACTS_REV` is unchanged, and `COMPAT.md` is not touched (Q-023
  records adoption).

```bash
cd /home/gui/projects/q/q_core
make check
cargo test -p q-indicators --test reference_gate -- --nocapture   # gate summary and bit-identity counts
cargo test -p q-indicators                                          # unit tests
make fixtures-check
make parity-isolation
make wheel-test

# negative control (step 12), not committed
make lint   # after inserting a mul_add call; expect a disallowed-method error, then git checkout -- crates/q-indicators

# human: benchmark (step 17)
cd /home/gui/projects/q/q_backend && uv run --with ../q_core/dist/q_core-*.whl python ../q_core/tools/bench/compare_pandas.py

# human: release (step 18), after merge
cd /home/gui/projects/q/q_core && make check && git tag vYYYY.MM.DD && git push origin vYYYY.MM.DD
uv run --no-project --with "q-core @ git+https://github.com/GuilhermeFortuna/q_core.git@vYYYY.MM.DD" \
  python -c "import numpy as np, q_core, q_core.indicators as qi; print(q_core.version(), qi.rsi(np.arange(1.0, 20.0), 14)[-1])"
```

## Handoff

Report the gate summary (16 bound, 0 pending) and the case count per function.
For each function, report the informational bit-identical and within-tolerance
counts, the largest absolute and relative differences seen for WMA and HMA,
and the traced cause of any other function's bit differences, with the
fixture's recorded CPU feature level. Report whether the numpy
crate built and imported under abi3, or which fallback was taken and why, and
the wheel filename showing its `abi3` tag. Report the exact lint error from the
`mul_add` negative control. List every `#[expect]` added, with its function.
Report every degenerate parameter behaviour that differed from this plan's
list, if any. From the benchmark, report the five runs and median per function
for pandas and `q_core`, and the speedup ratio of the medians. Report the new
workspace version, the tag pushed, and the output of the clean-environment
import from the tag.
