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

- [x] Add coverage for acceptance 2, 3 and 7 (bar range, invalid value and missing source).
- [x] Add `exit_price` (absent by default) to the decision inputs next to `amend_stop`.
- [x] Validate finite positive prices, source/OHLC availability and the bar's `[low, high]`.

### 2. Priced close after intrabar fills

**Files:** `crates/q-engine/src/candle/run.rs`.

- [x] Cover acceptance 1, 4, 5 and 6: long/short fills, protective precedence, missed prices and a same-bar resting fill.
- [x] After intrabar fills and before queued closes, scan stored trades from the later of bar open and fill time; consume the matching signal close even if the price is not reached.

### 3. Remove screen and tick callbacks

**Files:** `crates/q-engine/src/candle/run.rs`, `crates/q-py/src/engine.rs`, `README.md`, `tests/test_engine.py`, `crates/q-engine/tests/custom_intrabar_exits.rs`.

- [x] Delete the screen and tick paths and parameters; remove their tests.
- [x] Confirm acceptance 8 with a Python API test that removed keywords are rejected.

### 4. Documentation and checks

- [x] Rewrite the README section around `exit_price`.
- [x] Run `make check`; record: the initial run was blocked by unavailable network/tooling (fixture dependency download failed, maturin could not be provisioned, and Qt build did not finish). After fixing formatting, `cargo fmt --all --check` passed. `cargo clippy -p q-engine -p q-py --all-targets -- -D warnings`, `cargo test -p q-engine`, `cargo check -p q-py`, the focused Python binding smoke check and explicit-exit Rust tests passed. Full workspace/Qt checks and wheel-level Python tests remain unverified due those environment limits.
- [ ] Set the board task to in review with `./work board set Q-127 in-review -m "<summary, checks and results>"`. The human runs `./work finish Q-127` to release it.
