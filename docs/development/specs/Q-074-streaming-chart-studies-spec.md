# Q-074: Streaming chart studies

**Status:** plan awaiting review; status of record is the [Q project board](https://github.com/users/GuilhermeFortuna/projects/2)
**Depends on:** Q-022
**Consumer:** q_terminal Q-075 (live studies on the chart)
**Implementation plan:** [`../plans/Q-074-streaming-chart-studies-plan.md`](../plans/Q-074-streaming-chart-studies-plan.md)

## Purpose

The terminal chart shows indicator lines only for a deployment, and those values come
from a backend route. An operator watching a live symbol cannot put an EMA, Bollinger
bands, RSI, ATR or a session VWAP on the chart and watch it move with the forming bar.
Recomputing a batch kernel over the whole loaded window on every quote works, but it
scales with history length and gives no guarantee about what an incremental version
would compute.

Give `q-indicators` streaming study state that advances one completed bar at a time,
previews the forming bar without committing it, and is bit-identical to the batch
kernels that backtests use. Add a session VWAP with standard-deviation bands as a new
batch kernel with its streaming form. This task produces the Rust API and a released
`q_core` tag; the terminal wiring belongs to Q-075.

## Current system

`crates/q-indicators/src/window.rs` implements pandas-equivalent sequential kernels:
`rolling_mean` and `rolling_var` carry add/remove accumulators with compensation and a
consecutive-same-value count, and `ewm_mean` carries an `adjust=False` weighted state.
`sma`, `ema`, `bollinger_bands`, `rsi` and `atr` compose these primitives. Every kernel
is a pure function over a whole slice. Numeric families are gated against reference
fixtures exported from `q_backend` at `BACKEND_REV`; `q-parity` supplies tolerance,
double-run determinism and prefix-causality helpers.

## Required behaviour

- Add streaming state types for `sma`, `ema`, `bollinger_bands`, `rsi` and `atr`, with
  the same parameters and validation errors as their batch kernels. Each carries only
  the accumulators its batch kernel carries, plus the bounded window of past inputs a
  rolling removal needs.
- A state exposes three operations: commit one completed input and return its output;
  preview one forming input and return the output as if it were committed, leaving the
  state unchanged; and reset. Preview must not allocate after warm-up; a state may be
  cloned to preview.
- **Parity by construction.** For every prefix of every reference input, committing
  inputs one at a time yields outputs bitwise equal to the batch kernel over that prefix,
  NaN warm-up positions included. Previewing input `k` after committing `0..k` equals
  committing it. Refactoring the batch kernels to drive the streaming state is
  acceptable only if every existing reference fixture still passes unchanged.
- Add `session_vwap(high, low, close, volume, session)` returning VWAP and
  volume-weighted standard deviation of typical price `(h + l + c) / 3`. `session` is a
  caller-supplied integer key per bar; the accumulation resets whenever the key changes.
  Output is NaN while the session's cumulative volume is zero; negative or non-finite
  volume is an error. Bands at ±k·σ are a caller multiplication, not kernel state. Add
  its streaming form under the same parity rule. The kernel does no calendar or timezone
  arithmetic.
- Keep `q-indicators` free of I/O, clocks, environment access, Qt and Python. Do not
  change existing batch outputs, fixtures, Python bindings or wire contracts. Exposing
  `session_vwap` to Python is out of scope.

## Acceptance criteria

1. Streaming parity tests cover every reference input in `fixtures/reference/inputs/`
   for the five existing studies at their fixture parameters, prefix by prefix, bitwise,
   including NaN gaps, constant runs and periods longer than the input.
2. Preview/commit equivalence and preview-leaves-state-unchanged are tested for every
   study, including repeated previews of changing forming values.
3. `session_vwap` has hand-computed fixtures for one session, a mid-series session
   change, a zero-volume opening bar and invalid volume; streaming and batch agree
   bitwise; double-run determinism and prefix causality hold.
4. `make check` passes, and `./work finish Q-074` publishes a `vYYYY.MM.DD` tag that
   Q-075 can pin.
