# Q-127 implementation plan: Explicit exit price in the candle kernel

> **For implementation agents:** Read the linked specification and repository instructions. Launch only through `./work start Q-127 --agent <agent> --worktree` after written-plan approval.

**Goal:** a closed-bar decision can close at an exit price the market traded at during the bar; the screen and tick callbacks are removed.
**Spec:** [Specification](../specs/Q-127-explicit-exit-price-kernel-spec.md)
**Status:** written plan awaiting human review.

## Global constraints

- Next-open closes and every fill of stops, targets, breakeven and resting orders keep their current values.
- Validation errors are `CandleError` values that name the bar and column.
- No consumer outside `q_backend` uses the removed callbacks; check with `git grep` before deleting.

## Ordered implementation

### 1. Decision field and validation

**Files:** `crates/q-engine/src/candle/inputs.rs`, `decision.rs`, `run.rs`.

- [ ] Write failing tests for acceptance 2, 3 and 7.
- [ ] Add `exit_price` (NaN by default) to the decision inputs next to `amend_stop`.
- [ ] Validate it with the existing `check_positive`/`check_level` helpers and the bar's `[low, high]`. Commit.

### 2. Priced close after intrabar fills

**Files:** `crates/q-engine/src/candle/run.rs`.

- [ ] Write failing tests for acceptance 1, 4, 5 and 6.
- [ ] After the bar's intrabar fills and before closed-bar exits, scan the bar's trades from the later of the bar open and the fill time, using the existing intrabar price source, and close at the first trade that reaches the price. Commit.

### 3. Remove screen and tick callbacks

**Files:** `crates/q-engine/src/candle/run.rs`, `crates/q-py/src/engine.rs`, `README.md`, `tests/test_engine.py`, `crates/q-engine/tests/custom_intrabar_exits.rs`.

- [ ] Delete the screen and tick paths and their parameters. Remove or rewrite the tests that exercised them.
- [ ] Confirm acceptance 8 with a test that the parameters are rejected.
- [ ] Commit.

### 4. Documentation and checks

- [ ] Rewrite the README section on custom intrabar exits around `exit_price`.
- [ ] Run `make check`. Record the result in this plan.
- [ ] Set the board task to in review with `./work board set Q-127 in-review -m "<summary, checks and results>"`. The human runs `./work finish Q-127` to release it.
