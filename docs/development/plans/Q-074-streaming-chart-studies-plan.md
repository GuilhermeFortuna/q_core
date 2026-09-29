# Q-074 implementation plan: Streaming chart studies

> **For implementation agents:** Read the linked spec, `README.md`, and
> `RELEASING.md`. Work only on the task branch created by
> `./work start Q-074 --agent <agent> --worktree`. Track these steps with their
> checkboxes. Existing batch kernels and fixtures must not change output.

**Goal:** Incremental, forming-bar-aware study state in `q-indicators` that is
bit-identical to the batch kernels, plus a session VWAP.
**Architecture:** Each windowing primitive in `window.rs` gains a small state struct
holding exactly the accumulators its batch loop already carries. Study states compose
those structs the way the batch studies compose the functions. Preview clones the
state, commits the forming input and discards the clone.
**Tech stack:** Rust, `q-indicators`, `q-parity` (dev only).
**Spec:** [`../specs/Q-074-streaming-chart-studies-spec.md`](../specs/Q-074-streaming-chart-studies-spec.md)

## File map and interface

- `crates/q-indicators/src/window.rs`: `RollingMeanState`, `RollingVarState` and
  `EwmMeanState`, each with `push(x) -> f64`. Keep the batch functions' arithmetic in one
  place, either by driving them from the states or by sharing the `add_*` / `remove_*`
  helpers, so the two paths cannot drift.
- `crates/q-indicators/src/streaming.rs` (new): `SmaState`, `EmaState`,
  `BollingerState`, `RsiState`, `AtrState`, `SessionVwapState`. Each has
  `new(params) -> Result<Self, IndicatorError>`, `commit(input) -> Output`,
  `preview(&self, input) -> Output` and `reset()`. Output is `f64`, or a small `Copy`
  struct for multi-line studies.
- `crates/q-indicators/src/indicators.rs`: the batch `session_vwap`.
- `crates/q-indicators/tests/streaming_parity.rs` (new): parity against the existing
  reference inputs, reusing the loaders the reference tests already use.

## Ordered implementation

- [ ] **1. Primitive states.** Extract the loop state of `rolling_mean`, `rolling_var`
  and `ewm_mean` into state structs. The rolling states keep a ring of the last `window`
  prepared inputs for removal. Prove bitwise equality against the batch function for
  every prefix on the reference inputs before building on them. Run
  `cargo test -p q-indicators`.
- [ ] **2. Study states.** Compose SMA, EMA, Bollinger (mean plus std, same `ddof`), RSI
  (diff, gain/loss split, two EWM states) and ATR (previous close, true range, then its
  smoother) exactly as the batch functions do, including their `min_periods` and NaN
  handling. Add `preview` and `reset`. Test commit parity, preview/commit equivalence
  and state immutability under preview.
- [ ] **3. Session VWAP.** Implement batch `session_vwap` and `SessionVwapState` with
  cumulative `Σv·tp`, `Σv` and a numerically stable weighted-variance accumulator. Define
  the summation order once and use it on both paths. Add the hand-computed fixtures from
  the spec, plus determinism and prefix-causality checks through `q-parity`.
- [ ] **4. Docs and handoff.** List the streaming studies and `session_vwap` in the
  `q-indicators` row of `README.md`. Run `make check`. Commit focused changes and set
  Q-074 to In Review through `./work board set` with test results. The release tag is
  cut by `./work finish Q-074`.

## Review focus

- No study reaches parity by recomputing a window from scratch on each push. That
  would hide accumulator drift and lose the O(1) cost.
- Preview never mutates shared state, including the same-value counters and
  compensation terms.
- `session_vwap` resets on any change in the key, not only on an increase, and NaN
  inputs follow the documented rule on both paths.
