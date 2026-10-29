# Q-138 implementation plan: Live-placeable orders in the candle kernel

> **For implementation agents:** Read the linked specification and `q_core`'s `AGENTS.md`, `README.md` and
> `RELEASING.md`. Launch only through `./work start Q-138 --agent <agent> --worktree` after written-plan approval.

**Goal:** OCO groups of resting entries, timed exits and expiry, and a placement latency in the candle kernel.
Every existing order keeps its exact fills.
**Spec:** [Specification](../specs/Q-138-live-placeable-orders-in-the-candle-kernel-spec.md)
**Status:** written plan awaiting human review.

## Global constraints

- `q_core` only. `./work finish` releases it as a `vYYYY.MM.DD` tag, which `q_backend` pins in Q-139.
- Never edit an existing golden fixture or gate expectation. A changed one is a regression to report.
- In the worktree, symlink `target` to `/mnt/build/targets/q_core-<branch-slug>` before the first cargo build.

## Ordered implementation

### 1. Equivalence gates first

**Files:** `crates/q-engine/tests/live_placeable_equivalence.rs` (new), fixtures under
`fixtures/reference/candle_engine/` (new files only).

- [x] Write the three legacy forms of acceptance 2 against the current kernel, with fixture bars and ticks. Make
  them include a same-bar exit, a gap open and an exact touch of a level. Record their ledgers as the expected
  output.
- [x] Add the new forms as failing tests against those expected ledgers. Commit.

### 2. OCO groups

**Files:** `crates/q-engine/src/candle/inputs.rs`, `decision.rs`, `run.rs`, `protective.rs` as needed,
`crates/q-engine/tests/oco_groups_gate.rs` (new).

- [x] Write failing gates for the OCO cases in acceptance 3.
- [x] Generalise the single working entry to a group with first-fill cancellation, keeping the single resting
  entry as a group of one with identical fills. Apply the Q-132 rule to members.
- [x] Run all `q-engine` tests. Commit.

### 3. Timed orders and latency

**Files:** the same `candle/` modules, `crates/q-engine/src/candle/session_ticks.rs`,
`crates/q-engine/tests/timed_orders_gate.rs` (new).

- [x] Write failing gates for expiry by instant, the timed exit cases, and latency.
- [x] Implement them. With no instant and zero latency, the paths must be untouched. Commit.

### 4. Python surface

**Files:** `crates/q-py/src/engine.rs`, `tests/` Python tests.

- [ ] Expose the optional arguments. Add a test that a call without them returns byte-identical arrays to the
  previous release on the reference fixtures. Commit.

## Verification

- `make check`, including all `q-engine` and `q-parity` tests and the new gates.
- `./work board set Q-138 in-review -m "<summary; checks and results; follow-ups>"`.
