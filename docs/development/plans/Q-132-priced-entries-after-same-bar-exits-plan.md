# Q-132 implementation plan: Priced entries after same-bar exits in the candle kernel

> **For implementation agents:** Read the linked specification and repository instructions. Launch only through `./work start Q-132 --agent <agent> --worktree` after written-plan approval.

**Goal:** a priced entry on a bar where a trade closed intrabar fills only from a trade at or after that exit;
otherwise it is refused with a reason.
**Spec:** [Specification](../specs/Q-132-priced-entries-after-same-bar-exits-spec.md)
**Status:** written plan awaiting human review.

## Global constraints

- Bars with no intrabar exit keep every output bit for bit. Never edit an existing test's expected values to
  make a test pass.
- Scan ticks are compared by index in stored order, not by `time_us`, so trades with equal timestamps keep
  their order.
- Tests use hand-built series and intrabar sources, as `tests/priced_entries_gate.rs` does. No tick store, GPU
  or Qt build.

## Ordered implementation

### 1. Record the bar's last exit

**Files:** `crates/q-engine/src/candle/run.rs`.

- [ ] Return the scan tick index of the latest exit from `intrabar_fills` (`Option<usize>`).
- [ ] Have `priced_signal_exit` return the scan tick index at which it closed a trade, as well as whether it
  consumed the decision.
- [ ] In `run_bars`, keep the bar's last exit as the later of the two. It is reset each bar.

### 2. Touch from the last exit

**Files:** `crates/q-engine/src/candle/run.rs`, `crates/q-engine/src/candle/protective.rs`.

- [ ] Write failing tests for acceptance 1–7 in a new `tests/priced_entries_after_exit.rs`.
- [ ] Add a touch search that starts at a given tick and uses the first scanned trade as the reference side,
  with unit tests next to `touch_index`.
- [ ] When a last exit exists, have `open_priced_entry` find the touch before opening the trade. If there is
  none, push a reason-`1` refusal and open nothing. Otherwise open the trade, set its `first_tick` to the
  touch, and resolve its levels from the trades after it.
- [ ] Keep the current path, `touch_index` from the open and asked only when a level is in range, for bars with
  no last exit.

### 3. Rejection reason

**Files:** `crates/q-engine/src/candle/protective.rs`, `crates/q-py/src/engine.rs`, `tests/test_engine.py`.

- [ ] Add `reason: Vec<i8>` to `RejectedEntries`, and pass `0` from the existing wrong-side refusals.
- [ ] Return `rejected_reason` from the binding, and test acceptance 9 from Python.

### 4. Documentation and checks

- [ ] Update "Priced entries" in `README.md`: the last-exit rule, the touch from the exit tick, refusal instead
  of an error, and the reason codes.
- [ ] Run `cargo fmt --all --check`, `cargo clippy -p q-engine -p q-py --all-targets -- -D warnings`,
  `cargo test -p q-engine`, and the binding tests for the changed API. Record any check the environment
  blocked.
- [ ] Set the board task to in review with `./work board set Q-132 in-review -m "<summary, checks and results>"`.
  The human runs `./work finish Q-132` to release it.
