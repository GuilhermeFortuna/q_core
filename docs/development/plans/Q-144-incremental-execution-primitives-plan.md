# Q-144 — Incremental execution primitives implementation plan

> **For implementation agents:** Read the spec and repository instructions. After human plan approval,
> launch with `./work start Q-144 --agent <agent> --worktree`; use superpowers:executing-plans task by task.

**Goal:** release shared incremental matching and cost functions for Q-145 without changing research results.
**Architecture:** stateless Rust primitives with a thin PyO3 boundary; existing equivalent engine paths share
helpers while keeping quote and trade models explicit.
**Tech stack:** q-engine, q-py and PyO3; checked decimal arithmetic for currency conversion; no service framework.
**Spec:** [Q-144 specification](../specs/Q-144-incremental-execution-primitives-spec.md).
**Status:** proposed; awaiting human plan approval.

## Global constraints

- q_core only; no strategy code, wire protocol, simulator scheduler or terminal changes.
- Preserve existing numerical results; do not change expected goldens to make a refactor pass.
- Use normal date-tag release; Q-145 consumes the tag only after the human finishes this task.
- Create the worktree target symlink on `/mnt/build/targets/q-core-<branch-slug>` before any Cargo command.

## Review focus

- Exact touch differs by model: pin that distinction in native and binding fixtures.
- Stop gaps and limit improvement must not fill at an impossible order price.
- Boundary conversions must reject off-grid, overflowing and nonfinite input rather than round silently.
- Currency rounding occurs once; quantity steps and contracts are distinct units.
- Extracting shared code must retain all existing research behavior.

## Ordered implementation

### 1. Native primitives and shared helpers

**Files:** new `crates/q-engine/src/execution.rs`, `crates/q-engine/tests/incremental_execution.rs`;
modify `crates/q-engine/src/lib.rs`, `crates/q-engine/src/candle/protective.rs`,
`crates/q-engine/src/tick/simulate.rs`, and existing cost helper only where units agree.
**Interfaces:** produces the typed Rust equivalents of the three exact signatures in the spec; consumes
existing side types and cost arithmetic where compatible. q-py consumes these functions in step 2.

- [ ] Write failing table-driven tests for acceptance 1–3: exact touch, both gap directions, a limit with
  improvement, slippage, fees, bounds and repeated calls. Preserve quote/trade distinction explicitly.
- [ ] Run `cargo test -p q-engine --test incremental_execution` and confirm failures identify the missing API.
- [ ] Implement the primitives and extract equivalent comparisons/pricing from the existing engines. Keep
  old public entry points and result structures intact; add validated decimal conversions and cost output.
- [ ] Run `cargo test -p q-engine` (the affected numerical crate), plus `cargo fmt --all -- --check` and
  `cargo clippy -p q-engine --all-targets -- -D warnings`. Commit the implementation and tests.

### 2. Python boundary and capability

**Files:** new `crates/q-py/src/execution.rs`, `tests/test_incremental_execution.py`; modify
`crates/q-py/src/lib.rs` (native submodule registration), `Makefile` (wheel-test includes the new script),
affected Cargo dependency manifests/lockfile if a decimal dependency is needed, and `README.md`.
**Interfaces:** consumes step 1; exports the exact `q_core.execution` functions and capability from the spec.

- [ ] Write failing Python fixture tests for every numerical vector, decimal conversion, exceptions and capability.
- [ ] Build with the documented maturin invocation (`maturin build --release --manifest-path
  crates/q-py/Cargo.toml --out /mnt/build/wheels/q-144`) and link worktree `dist` to that wheel directory.
  Do not use `make wheel` here: its removal of `dist` would remove the link. Keep temporary test environments
  on the build disk with `TMPDIR=/mnt/build/tmp/q-144`. Implement the thin wrappers, run
  `python3 tests/test_incremental_execution.py`, `python3 tests/test_tick_engine.py` and
  `python3 tests/test_engine.py` through their existing wheel-test harness pattern.
- [ ] Run format and lint for q-py and Python files touched; document price models, units, rounding and examples.
- [ ] Commit; report the actual focused commands/results through `./work board set Q-144 in-review -m "…"`.
  Do not release or push. Human `./work finish Q-144` releases the capability required by Q-145.
