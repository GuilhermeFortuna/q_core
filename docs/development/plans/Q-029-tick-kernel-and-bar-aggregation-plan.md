# Q-029 implementation plan: Tick kernel and bar aggregation

**Status:** authoritative in the [Q project board](https://github.com/users/GuilhermeFortuna/projects/2)  
**Specification:** [`../specs/Q-029-tick-kernel-and-bar-aggregation-spec.md`](../specs/Q-029-tick-kernel-and-bar-aggregation-spec.md)  
**Depends on:** Q-021

## Current-system context

`q-engine` holds whatever Q-026 and Q-027 have merged by the time this task
starts; this task adds a sibling `tick` module and touches nothing in `exits` or
`candle`. Backend reference families follow the `bar_window` template
(`environment = "backend"`, exact policy, blob-id provenance, listed in
`BACKEND_FAMILIES`), and `BACKEND_REV` is `067e29c`. `q-py` projects arrays with
the `contiguous()` helper pattern from `src/indicators.rs`.

In `q_backend` at the pin, `backtesting/tick/kernel.py` defines
`_compute_quantity` (line 17: fixed quantity returns `sizing_a` unchanged; safety
margin does `int(capital / sizing_a)`, caps at `int(sizing_c)` when
`sizing_c > 0.0`, then `if qty < int(sizing_b): qty = min if min > 0 and qty == 0
else return 0.0`) and `_simulate_njit` (line 46, `@njit(cache=True)`, default
`fastmath=False`). The loop keeps one position: while flat, a non-zero
`direction[i]` sizes with current capital and opens long at `ask[i]` or short at
`bid[i]`, taking `sl_points[i]`/`tp_points[i]` (NaN disables) to fix
`sl_price`/`tp_price`; while in a position it fills at `bid` (long) or `ask`
(short) and exits on `sl` (`<=`/`>=`), then `tp`, then opposite direction, with
`pnl = (fill - entry) * qty * point_value` added to capital. The `if/else` on
`position_dir == 0` means the exit tick cannot re-enter, and `block_reentry`
(lines 78-82, 160) is set and cleared without being read. A position open after
the last tick closes at `bid[n-1]`/`ask[n-1]` with `END_OF_DAY` (4). The function
returns seven length-`n` arrays, `trade_count` and `final_capital`.
`tick/orders.py` holds `ExitReason` (1-4) and `kernel_sizing_params`, which reads
`config.safety_margin_per_contract` for anything that is not fixed quantity, so an
`InverseVolatilityPositionSizing` raises `AttributeError`. `tick/engine.py`
`_split_ticks_by_day` (line 20) cuts at changes of `time_msc // 86400000`, and
`TickBacktestEngine._run_single_chunk` computes signals per chunk and simulates
each with `initial_capital`. `tick/chart_data.py` `_mid_price` (line 40) uses
`last` if `np.any(last > 0)` else `(bid + ask) / 2.0`; `_resample_ticks_to_bars`
(line 61) cuts at changes of `time_msc // bar_ms` and, in a Python loop per bar,
builds `open = prices[start]`, `high = np.max`, `low = np.min`,
`close = prices[end - 1]`, `volume = int(np.sum(volume[start:end]))`, and records
`(start, end)`; `_sample_indicator_at_bars` (line 95) takes `series[end - 1]` with
NaN as `None`; `_resolve_bar_ms` (line 47) doubles `bar_ms` while
`span // bar_ms > 50_000` and breaks after a doubling that exceeds `D1`. Tests are
`tests/backtesting/tick/test_kernel.py` (9), `test_tick_engine.py` (3),
`test_tick_chart_data.py` (2) and `test_tick_strategy_causality.py`. The gap is
that the tick simulation and its neighbours are outside `q_core`, and nothing
pins them value for value.

## Interfaces produced

```rust
// crates/q-engine/src/lib.rs   (changed)
pub mod tick;
pub use tick::{resolve_bar_ms, sample_at_bar_ends, simulate_ticks, tick_bars, tick_day_bounds,
               TickBars, TickError, TickExitReason, TickInputs, TickLedger, TickRun, TickSizing};
```

```rust
// crates/q-engine/src/tick/simulate.rs   (new)
#[derive(Clone, Copy, Debug)]
pub enum TickSizing {
    FixedQuantity { quantity: f64 },                                        // used as given, never floored
    FixedSafetyMargin { margin_per_contract: f64, min_contracts: i64, max_contracts: i64 },  // max 0 = none
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TickExitReason { StopLoss = 1, TakeProfit = 2, Signal = 3, EndOfDay = 4 }

pub struct TickInputs<'a> {
    pub bid: &'a [f64], pub ask: &'a [f64],
    pub direction: &'a [i8],                // sign only
    pub sl_points: &'a [f64], pub tp_points: &'a [f64],   // NaN = none
}

#[derive(Clone, Debug, Default)]
pub struct TickLedger {
    pub entry_idx: Vec<i64>, pub exit_idx: Vec<i64>,
    pub entry_price: Vec<f64>, pub exit_price: Vec<f64>,
    pub direction: Vec<i8>, pub quantity: Vec<f64>,
    pub exit_reason: Vec<i64>,              // TickExitReason codes
}
pub struct TickRun { pub trades: TickLedger, pub final_capital: f64 }

#[derive(Clone, Debug, PartialEq)]
pub enum TickError {
    LengthMismatch { column: &'static str, expected: usize, actual: usize },
    InvalidSizing { field: &'static str, reason: &'static str },
    VolumeNotFinite { bar: usize },          // int() of a NaN or infinite sum raises today
}

pub fn simulate_ticks(inputs: &TickInputs<'_>, initial_capital: f64, point_value: f64,
                      sizing: TickSizing) -> Result<TickRun, TickError>;
```

```rust
// crates/q-engine/src/tick/days.rs   (new)
/// (starts, ends) of consecutive runs of equal floor(time_msc / 86_400_000).
pub fn tick_day_bounds(time_msc: &[i64]) -> (Vec<i64>, Vec<i64>);
```

```rust
// crates/q-engine/src/tick/bars.rs   (new)
#[derive(Clone, Debug, Default)]
pub struct TickBars {
    pub open_msc: Vec<i64>,                 // bucket * bar_ms
    pub open: Vec<f64>, pub high: Vec<f64>, pub low: Vec<f64>, pub close: Vec<f64>,
    pub volume: Vec<i64>,                   // trunc(numpy-order sum)
    pub tick_start: Vec<i64>, pub tick_end: Vec<i64>,   // [start, end)
}
pub fn tick_bars(time_msc: &[i64], bid: &[f64], ask: &[f64], last: &[f64], volume: &[f64],
                 bar_ms: i64) -> Result<TickBars, TickError>;
pub fn resolve_bar_ms(base_bar_ms: i64, span_msc: i64) -> i64;
pub fn sample_at_bar_ends(series: &[f64], tick_end: &[i64]) -> Vec<f64>;   // NaN = missing

// crates/q-engine/src/tick/numpy_sum.rs   (new, crate-private)
pub(crate) fn numpy_sum_f64(values: &[f64]) -> f64;   // numpy's float64 add.reduce order
```

```python
# crates/q-py/src/engine.rs   (changed, or new if Q-027 has not merged) -> q_core.engine
def tick_simulate(*, bid: NDArray[float64], ask: NDArray[float64], direction: NDArray[int8],
                  sl_points: NDArray[float64], tp_points: NDArray[float64], initial_capital: float,
                  point_value: float, sizing: Mapping[str, object]) -> dict[str, NDArray | float]: ...
                  # keys: TickLedger field names and "final_capital"
def tick_day_bounds(time_msc: NDArray[int64]) -> tuple[NDArray[int64], NDArray[int64]]: ...
def tick_bars(*, time_msc: NDArray[int64], bid: NDArray[float64], ask: NDArray[float64],
              last: NDArray[float64], volume: NDArray[float64], bar_ms: int) -> dict[str, NDArray]: ...
def resolve_bar_ms(base_bar_ms: int, span_msc: int) -> int: ...
def sample_at_bar_ends(series: NDArray[float64], tick_end: NDArray[int64]) -> NDArray[float64]: ...
```

```
crates/q-engine/tests/tick_gate.rs              new: tick_kernel and tick_bars gates, negative controls, determinism
tools/reference/families/tick_kernel.py         new: backend family (simulate and _split_ticks_by_day)
tools/reference/families/tick_bars.py           new: backend family (_resample_ticks_to_bars, _sample_indicator_at_bars, _resolve_bar_ms)
fixtures/reference/tick_kernel/                 new: 12 fixture files
fixtures/reference/tick_bars/                   new: 8 fixture files
tests/test_tick_engine.py                       new: wheel test
tests/bench_tick_kernel.py                      new: human benchmark against the pinned numba kernel
Makefile                                        BACKEND_FAMILIES += tick_kernel tick_bars; bench-tick-kernel target; wheel-test runs test_tick_engine.py
README.md                                       fixture protocol and benchmark lines
```

## Implementation decisions

- **A separate `tick` module with its own sizing enum.** The tick kernel's fixed
  quantity is not floored and its safety margin truncates toward zero, so
  `int(-0.2)` is 0 and takes the minimum-contracts branch, where Q-027's candle
  sizer floors to -1 and does not. One sizing type would force one of the two
  behaviours on both engines.

- **The numba integer conversions are reproduced as `f64::trunc` then `as i64`,
  with min and max held as `i64`.** `int(x)` in a numba function truncates toward
  zero into int64, and `int(sizing_b)`/`int(sizing_c)` truncate the config floats.
  Every reachable value is far inside `i64`. Holding counts as `i64` keeps
  `qty == 0` and `qty < min_c` integer comparisons, as numba compiles them.

- **The loop is a line-for-line port of `_simulate_njit`, including its dead
  `block_reentry` flag's absence of effect.** The `if position_dir == 0 ... else`
  structure is what prevents re-entry on an exit tick, so the port keeps that
  structure rather than adding a flag. Levels keep numba's expression order
  (`entry - sl_pts`, `((fill - entry) * qty) * point_value`). numba compiles with
  `fastmath=False`, so LLVM does not contract multiply-add, and the port must not
  either; `suboptimal_flops` is allowed with a reason on the pnl and level
  functions only.

- **The ledger is exact-length columns plus final capital, not `n`-length arrays
  with a count.** The numba return shape exists because njit cannot grow arrays
  cheaply. Q-030 slices to `trade_count` today, so exact-length columns remove a
  step without changing any value.

- **Sizing mappings are the backend's `PositionSizingConfig.model_dump()`, and an
  inverse-volatility config is a `ValueError` naming `inverse_volatility`.**
  Today it raises `AttributeError` from `kernel_sizing_params`. The tick path
  cannot run it either way, and a named error is the smallest honest change, made
  at the boundary rather than inside the kernel. Q-030 decides whether the
  backend's own error changes and records it.

- **Floor division uses `div_euclid`.** numpy's `//` on int64 floors, and a
  positive divisor makes Euclidean and floor division identical, including for
  negative timestamps that plain `/` would truncate toward zero.

- **The midpoint-or-last choice is made once over the whole input, as today.**
  `np.any(last > 0)` is false for NaN, so a stream with NaN and non-positive last
  prices uses the midpoint. The port evaluates the same predicate over the same
  slice.

- **High and low propagate NaN as `np.max`/`np.min` do.** Rust's `f64::max`
  ignores NaN, which would turn a NaN high into a number. The bar fold returns NaN
  as soon as it sees one, which is numpy's behaviour.

- **Volume is summed in numpy's pairwise order.** `np.sum` over float64 uses
  pairwise summation (sequential below 8 elements, eight running sums up to a
  block of 128, recursive halving above), and a left-to-right sum can round
  differently. The value is truncated afterwards, which hides most differences,
  but not at integer boundaries, and the exact policy compares the truncated
  integer. `numpy_sum_f64` implements numpy's order and is unit-tested against
  values recorded from numpy for lengths 1, 7, 8, 9, 127, 128, 129, 1000 and
  20,000; the `b04` fixture includes bars of those lengths so the gate, not the
  unit test, is the authority.

- **A non-finite volume sum is an error.** `int(nan)` raises `ValueError` and
  `int(inf)` raises `OverflowError` in the resampler today, so the chart request
  fails. Returning a number would silently change a failure into a result.

- **`resolve_bar_ms` ports the loop including its break after the doubling.**
  `H12` doubles to `D1` (not greater, continue) and then to `2 * D1` (greater,
  break), so the interval can exceed one day. The `b05` fixture pins it.

- **Timeframe names and ISO timestamps stay in Python.** `DISPLAY_TIMEFRAME_MS`
  and the `ValueError` listing valid names are request validation, and
  `_msc_to_iso` goes through Brasília wall-clock conversion in
  `market_data/timezone.py`. Neither is market-data iteration, and moving them
  would put timezone policy in `q_core`, which Q-025 keeps out.

- **Fixtures drive the backend functions directly: `simulate`,
  `_split_ticks_by_day`, `_resample_ticks_to_bars`, `_sample_indicator_at_bars`
  and `_resolve_bar_ms`.** These are exactly what Q-030 replaces, and numba is
  only installed in the full backend environment, so both families are backend
  families. The exporter slices numba's arrays to `trade_count`.

- **Simulation scenarios (12 files):**
  - `t01_sl_before_tp`, `t02_tp_only` and `t03_opposite_signal_no_reentry` reuse
    the shapes of `test_kernel.py`'s cases;
  - `t04_long_spread_loss` and `t05_short_spread_loss`;
  - `t06_end_of_stream_close`;
  - `t07_nan_levels` (NaN stop and target on the entry tick, finite later);
  - `t08_fractional_fixed_quantity` (quantity 1.5);
  - `t09_safety_margin_min_max` (margin 2,000, min 1, max 3, compounding);
  - `t10_negative_capital` (a loss that drives capital below zero, min 1);
  - `t11_synthetic_stream` (`test_goldens.synthetic_ticks()` with `TickMaBreakout`
    short 20, long 50, stop 0.3, target 0.6, directions exported as inputs);
  - `t12_day_bounds` (three UTC days, a single day, an empty stream, and a
    pre-1970 timestamp).
- **Bar scenarios (8 files):**
  - `b01_last_price_m1`;
  - `b02_midpoint_when_no_last`;
  - `b03_nan_price_in_bar`;
  - `b04_pairwise_volume_lengths`;
  - `b05_interval_doubling` (sparse ticks spanning 40 days at M1, and a
    5,000,000,000,000 ms span at H12 that doubles past `D1`);
  - `b06_indicator_sampling_nan`;
  - `b07_out_of_order_ticks` (a repeated bucket);
  - `b08_synthetic_stream_m5`.

## Ordered implementation

- [x] 1. Work on the branch `Q-029-tick-kernel-and-bar-aggregation` in `q_core`,
   created from `development` by `./work start`. If Q-026 or Q-027 has merged,
   reuse the `q-engine` dev-dependencies and `q-py/src/engine.rs` they added
   instead of adding them again. Confirm `BACKEND_REV` is `067e29c` and that the
   backend's `backtesting/tick/` is unchanged between the pin and `development`;
   otherwise stop and report.
- [x] 2. Write failing tests in `tick/numpy_sum.rs` against sums recorded from numpy
   (record the bits, computed once with `np.sum` on a fixed-seed
   `rng.random(n)`) for the nine lengths in the decisions, and for `[-0.0]`,
   asserting the sign bit numpy actually recorded rather than an assumed one.
   Confirm they fail. Implement. Confirm they pass. Commit.
- [x] 3. Write failing tests in `tick/simulate.rs`:
   - long entry at ask 100.0, stop 0.5, a later bid of exactly 99.5 exits with
     reason 1 at 99.5 (touching the level);
   - with both stop and target reached on one tick the reason is 1;
   - opposite direction on tick 3 exits with reason 3 and opens nothing on tick 3,
     and a direction on tick 4 opens;
   - NaN stop on the entry tick and 0.5 on the next tick never stops;
   - a position open at the end closes at the last bid with reason 4;
   - fixed quantity 1.5 trades 1.5;
   - safety margin 2,000 with capital 4,500, max 0 trades 2, with max 1 trades 1;
     capital -500 with min 1 trades 1; capital 3,000 with min 2 trades nothing;
   - a length mismatch names `ask`.

   Confirm they fail. Implement `simulate_ticks`. Confirm they pass. Commit.
- [x] 4. Write failing tests in `tick/days.rs` and `tick/bars.rs`:
   - day bounds for times across two UTC midnights give three runs, and a time of
     -1 ms falls on the day before 1970-01-01;
   - bars with `last` all 0.0 use the midpoint, with one `last` of 0.01 use `last`
     everywhere;
   - a NaN price makes that bar's high and low NaN but not its open;
   - volumes `[0.5, 0.6]` give 1;
   - a NaN volume is `VolumeNotFinite`;
   - `resolve_bar_ms(60_000, 3_456_000_000)` (40 days at M1) returns `120_000`;
     `resolve_bar_ms(43_200_000, 5_000_000_000_000)` returns `172_800_000`,
     one doubling past `D1`; `resolve_bar_ms(60_000, 0)` returns `60_000`;
   - `sample_at_bar_ends` returns the value at `end - 1`.

   Confirm they fail. Implement. Confirm they pass. Commit.
- [x] 5. Add exporter unit tests for `tick_kernel.py` and `tick_bars.py`: the ledger
   encoder slices to `trade_count`; the bar encoder writes `None` samples as NaN;
   the scenario builder for `b07` really produces a repeated bucket. Confirm they
   fail. Write both families with the scenarios in the decisions and register
   them. Confirm they pass. Commit.
- [x] 6. In the full backend environment run `make fixtures-backend`. Confirm that only
   `tick_kernel/` and `tick_bars/` appeared, that `t01` to `t10` each show the edge
   they name in their ledger, and that `b04` contains bars of every listed length.
   Commit the 20 files.
- [x] 7. Write `tests/tick_gate.rs`: accounting for both families; per scenario rebuild
   inputs, run, and compare every column and `final_capital` under the exact
   policy. Confirm it fails on encoding first and fix encoding only; a value
   difference is a kernel defect that gets a failing unit test in step 2, 3 or 4's
   module first. Confirm it passes. Commit.
- [x] 8. Add negative controls (ULP price, exit tick +1, ULP final capital, bar volume
   +1, checksum, unaccounted file) and the double-run check per scenario. Confirm
   they pass. Commit.
- [x] 9. Write failing wheel tests in `tests/test_tick_engine.py`: `tick_simulate` on
   `t09` inputs equals the fixture; `tick_day_bounds` and `tick_bars` equal `t12`
   and `b01`; a float32 `bid`, a 10-long `ask` against an 11-long `bid`, and
   `{"type": "inverse_volatility", ...}` each raise `ValueError` naming `bid`,
   `ask` or `inverse_volatility`; `direction` of dtype int64 raises `TypeError`.
   Confirm they fail. Implement the projections. Confirm they pass under
   `make wheel-test`. Commit.
- [x] 10. Add `tests/bench_tick_kernel.py` and the `bench-tick-kernel` target, cloning
    the backend at `BACKEND_REV` as the candle benchmark does: time
    `simulate` on a 5,000,000-tick stream once in a fresh process (compilation
    included, with the numba cache directory pointed at a temporary directory),
    then five warm runs, and five runs of `q_core.engine.tick_simulate`, asserting
    equal ledgers first. Commit.
- [x] 11. Update README lines. Confirm existing gates pass unchanged and
    `make parity-isolation` passes. Release preparation: bump the workspace and
    `pyproject.toml` versions to today's date per `RELEASING.md` step 2 (and the
    Qt harness's expected version), so the human can tag the merge that Q-030
    pins. Commit.
- [ ] 12. Human step, matching human-verifiable criterion 1: `time make fixtures-backend-check`.
- [ ] 13. Human step, matching human-verifiable criterion 2: `make bench-tick-kernel`.
- [x] 14. Run the full validation suite. Commit.

## Validation

- **Unit:** numpy pairwise sum order; stop before target, touch exits, no
  re-entry on the exit tick, entry-tick levels, NaN levels, end-of-stream close,
  unfloored fixed quantity, truncating safety margin with min, max and negative
  capital; UTC day runs including pre-1970; midpoint-or-last choice; NaN high and
  low; volume truncation and non-finite volume; interval doubling; sampling at bar
  ends.
- **Integration:** 12 tick-kernel and 8 tick-bar fixtures through the Q-021 gate
  under the exact policy; wheel round trips.
- **Regression:** all existing gates unchanged; `make parity-isolation`.
- **Negative controls:** ULP price, exit tick shift, ULP final capital, volume
  change, checksum, unaccounted file.
- **Determinism:** double run per scenario.
- **Measurement:** numba first call and warm runs against the kernel on 5,000,000
  ticks.
- **Manual:** fixture regeneration in the full backend environment.

```bash
cd /home/gui/projects/q/q_core
make check
cargo test -p q-engine --lib tick
cargo test -p q-engine --test tick_gate -- --nocapture
uv run --frozen --project tools/reference python -m unittest discover -s tools/reference
make wheel-test
make parity-isolation

# human (steps 12 and 13)
time make fixtures-backend-check
make bench-tick-kernel
```

## Handoff

Report the 20 scenario ids with each simulation scenario's trade count and exit
reasons and each bar scenario's bar count. Report the recorded numpy sums used by
the unit tests and whether any bar in `b04` would have differed under a
left-to-right sum. Report every value that first differed from a fixture, its
cause, and the unit test that pins it. List every `suboptimal_flops` allowance.
Report negative controls and determinism results and confirm `BACKEND_REV` is
unchanged. From the human steps, report the regeneration diff and time, and the
numba first-call time, warm individual and median times, and the kernel's
individual and median times.
