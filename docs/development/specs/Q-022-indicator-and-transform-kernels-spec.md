# Q-022: Indicator and transform kernels

**Status:** authoritative in the [Q project board](https://github.com/users/GuilhermeFortuna/projects/2)  
**Project direction:** [`q_contracts/docs/system-architecture.md` §5, §9 invariants 1 and 5, §10 phase 2](https://github.com/GuilhermeFortuna/q_contracts/blob/f2273a88e52c5b9a8ad5ac9d7eb27f8069cf643d/docs/system-architecture.md#10-roadmap)  
**Depends on:** Q-021  
**Implementation plan:** [`../plans/Q-022-indicator-and-transform-kernels-plan.md`](../plans/Q-022-indicator-and-transform-kernels-plan.md)

## Purpose

Every indicator that research backtests and live evaluation use is computed by
pandas in `q_backend`. There are 16 of them: 7 technical indicators, 5 moving
average types, and 4 series transforms. `q_core`'s indicator crate is still an
empty skeleton, so the architecture's rule that shared semantics exist once, in
Rust, has nothing to point at. Q-021 fixed a target for these functions:
reference fixtures exported from today's Python, marked pending, and a gate that
checks goldens, double-run determinism, and prefix causality. This task
implements the 16 functions in Rust with pandas' exact semantics. Numerical
details such as warm-up placement, missing-value handling, and pandas' running
sum and variance updates are copied, not approximated. It turns on all 16
fixtures and projects the functions into the Python wheel as numpy array
functions. It then cuts a `q_core` release, so that Q-023 can make the backend
delegate to it without the backtest goldens moving.

## Requirements

### Functions and semantics

- `q_core` provides the 16 functions Q-021 fixed: realized volatility,
  Yang–Zhang volatility, RSI, Bollinger bands, MACD, Donchian channels, ATR, the
  SMA, EMA, SMMA, WMA and HMA moving averages, rolling z-score, rolling rank,
  percent change, and clip.
- Each function returns what the backend's Python implementation at the Q-021
  pinned commit returns for the same inputs and parameters. This holds under
  the comparison policy each fixture declares: missing-value positions exact,
  infinities exact, and finite values within the declared tolerance. For the
  indicators that tolerance is an absolute bound of 1e-10 or a relative bound of
  1e-12 of the expected value, whichever is looser.
- The reproduced behavior includes the parts that look like accidents. Rolling
  z-score uses the sample standard deviation. Wilder smoothing uses pandas'
  center-of-mass form of `1/period`. The exponential averages carry the last
  value across a missing input. Rolling windows treat infinities as missing,
  and rolling rank does not. Donchian channels are shifted one bar. HMA uses
  integer halving and a truncated square root. Constant windows produce the
  exact values pandas produces.
- Output length always equals input length. A missing value is represented as
  NaN on both sides of the Python boundary, and no other sentinel is used.
- Functions whose outputs are several series return all of them together, in
  the order and with the meaning the Python functions give them.

### Parameters and rejection

- A function rejects exactly the parameter values for which the Python
  implementation raises, and computes for every value where Python returns a
  result, including degenerate ones. In particular:
  - RSI and ATR reject a period of zero or less.
  - Yang–Zhang rejects a negative window and a window of one, and returns all
    missing values for a window of zero.
  - Realized volatility, Bollinger bands, Donchian channels, and rolling z-score
    reject negative windows and return all missing values for a window of zero.
  - The five moving averages reject periods below one.
  - MACD rejects any span below one.
  - Rolling rank rejects a window of zero or less when the input is non-empty,
    and returns an empty result for an empty input.
  - Percent change rejects a lag below one.
  - Clip rejects a lower bound above the upper bound. A missing bound leaves
    that side unclipped.
- A rejection is a distinguishable error that names the function, the parameter,
  and the offending value. It is never a panic, an abort, or an all-missing
  result. Python's exception types (`ZeroDivisionError`, `IndexError`,
  `ValueError`) are not reproduced. Every rejection surfaces in Python as
  `ValueError` with a message naming the parameter. For the inputs where the
  reference raised `ZeroDivisionError` or `IndexError`, this is the one
  intended behavior change, and it is accepted.
- Inputs that must be the same length (the price columns of Yang–Zhang and ATR)
  are rejected when they are not. They are never truncated or padded.

### Determinism and causality

- Every function passes Q-021's double-run determinism check with bit-identical
  results, and its prefix-causality check on every case.
- Results do not depend on thread count, CPU feature detection, or allocation.
  The functions use no parallelism, no fused multiply-add, and no reordering of
  floating-point sums.
- The functions keep the semantic crate's purity rules: no input or output, no
  clock, no environment, no unsafe code.

### The gate

- All 16 Q-021 fixtures are bound to their kernels, and none is listed as
  pending. The gate reports 16 bound and 0 pending.
- No fixture, tolerance, or comparison policy is changed to make a kernel pass.
  A kernel that cannot meet a fixture is a failed task, and it is reported as
  one.

### Python projection

- The wheel exposes the 16 functions to Python. Each takes one-dimensional
  float64 numpy arrays and plain numeric parameters, and returns new float64
  numpy arrays. Multi-output functions return a tuple.
- The projection creates no per-element Python objects, imports no pandas, and
  makes no copy of an input array. Each output reaches Python without being
  copied again after it is computed.
- An input with the wrong dtype, dimensionality, or memory layout is refused
  with a clear error. It is never silently cast or copied.
- The kernels are float64-only. Restoring a caller's dtype (for example, the
  integer dtype pandas keeps when clipping integers with whole-number bounds),
  its index, or its series name is the caller's job.
- Input arrays are not modified.
- The projection contains no semantics. Each binding converts arrays, calls the
  kernel, and converts errors.
- The existing `version()` and `contracts_rev()` callables keep their names and
  results.

### Release

- A `q_core` release tag in the `vYYYY.MM.DD[.n]` form is cut from the merged
  work according to `RELEASING.md`. The wheel's reported version equals the tag's
  date.
- The release procedure states every step it needs, including any version
  change, so that the next release does not rediscover it.

## Constraints and non-goals

- **No `q_backend` change.** The backend keeps computing with pandas until Q-023.
  Delegation, pinning the wheel, Python-side parameter validation with today's
  messages, and dtype and index restoration all belong to Q-023.
- **No streaming or incremental indicator state.** An `update(bar)` form of each
  indicator is the obvious next step for live evaluation, but the evaluator's
  window semantics arrive with Q-025's bar frames and the evaluator swap in
  Q-031. These functions compute over whole arrays.
- **No faster algorithms that change results.** Rolling rank keeps its
  window-scan definition. Rolling mean and variance keep pandas' add-and-remove
  running updates, and are not recomputed per window or vectorized. WMA keeps a
  sequential weighted sum. Speed is measured and reported, not tuned.
- **No attempt to reproduce numpy's BLAS dot-product bits.** NumPy's WMA uses an
  OpenBLAS kernel that depends on the CPU. On the development machine it
  differs from a sequential sum by at most 7.1e-14 for windows of 16 or more.
  The fixture's relative bound covers this difference at any price scale,
  including WIN$N's roughly 130,000 points, where the absolute bound alone would
  be tight. Matching one CPU's kernel would make the Rust result depend on the
  CPU.
- **No kernels beyond the 16.** The inline indicator copies in the genome
  composite strategy and the feature store, the numba code in time-series
  momentum and pairs trading, session and exogenous context, and the tick
  strategy's rolling SMA are not ported here.
- **No Qt projection of the indicators.** `q_terminal` has no consumer for them
  until phase 3.
- **No new comparison policy, fixture, or harness capability.** Those are
  Q-021's. This task binds kernels through the harness as it is.
- **No `COMPAT.md` change.** The release is recorded there when a consumer
  adopts it, which is Q-023.

## Acceptance criteria

### Agent-verifiable

1. The Q-021 gate reports 16 bound and 0 pending, and every case of every
   fixture passes: goldens under the fixture's policy, rejection wherever the
   reference rejected, double-run determinism, and prefix causality.
2. For each function, the number of finite expected values that are
   bit-identical and the number that are only within tolerance are reported.
   The report is informational and not a gate. Any function other than WMA and
   HMA that is not fully bit-identical is named, with its cause. A cause may be
   a platform difference in the numerical libraries, judged against the CPU
   feature level recorded in the fixture's provenance.
3. Unit tests pin the reproduced pandas behaviors with values taken from pandas
   3.0.2: the value carried across a missing input by the exponential averages,
   including the center-of-mass-one case; infinities treated as missing in
   rolling windows but not in rolling rank; exact results for constant windows;
   the variance recomputation after a catastrophic cancellation; the one-bar
   Donchian shift; HMA's first valid index; and each degenerate parameter listed
   under Requirements.
4. Every parameter listed as rejected returns an error that names the function,
   parameter, and value, and no kernel panics on any fixture input.
5. Mismatched price column lengths are rejected for Yang–Zhang and ATR.
6. Introducing a fused multiply-add call into the indicator crate fails lint. The
   change is reverted.
7. In a clean environment, the installed wheel exposes the 16 functions. Each
   returns float64 arrays of the input length. Hand-checked values match, a
   rejected parameter raises, a non-float64 input and a non-contiguous input are
   refused, an input array is unchanged after the call, and no returned array
   shares memory with an input.
8. `q_core.version()` equals the workspace version, and the workspace version
   equals the release date that the tag will carry.
9. `RELEASING.md` lists every step taken to cut this release.
10. The full validation suite passes, including the Q-021 staleness check.

### Human-verifiable

1. The release tag is cut on the merged commit and pushed, and a clean
   environment resolves the wheel from the tag and runs an indicator.
   Command: `git -C q_core tag "v$(grep -m1 '^version' q_core/Cargo.toml | cut -d'"' -f2 | awk -F. '{printf "%s.%02d.%02d", $1, $2, $3}')" && git -C q_core push origin --tags && uv run --no-project --with "q-core @ git+https://github.com/GuilhermeFortuna/q_core.git@$(git -C q_core describe --tags --abbrev=0)" python -c "import numpy as np, q_core, q_core.indicators as qi; print(q_core.version(), qi.rsi(np.arange(1.0, 20.0), 14)[-1])"`
   (expected output: the tag's version and `100.0`)
2. The kernels' run time is compared against the backend's pandas functions on
   one million bars (100,000 for rolling rank), with five runs each. Individual
   times and medians are reported for RSI, Bollinger bands, WMA, HMA, Yang–Zhang,
   and rolling rank.
   Command: `cd q_backend && uv run --with ../q_core/dist/q_core-*.whl python ../q_core/tools/bench/compare_pandas.py`
3. The task branch's CI run on GitHub passes.
   Command: `gh run watch -R GuilhermeFortuna/q_core`
