# Q-102 implementation plan: Intrabar stop and target orders in the candle kernel

> **For implementation agents:** Read the linked spec and repository instructions.
> Use superpowers:executing-plans when that skill is available. Start only through
> `./work start Q-102 --agent <agent> --worktree` after written-plan approval.
> Implement this task natively; delegation requires separate authorization.

**Goal:** An entry can carry a stop and a target price that the candle kernel fills inside the bar, loading a bar's trade prices only when its range reaches a level.
**Architecture:** `q-engine` adds one step to the bar loop between sections C and D, fed by optional level columns and a host-supplied `IntrabarSource`; `q-py` projects both.
**Spec:** [Specification](../specs/Q-102-intrabar-stop-and-target-orders-in-the-candle-kernel-spec.md)
**Status:** written plan awaiting human review.

## Global constraints

- The linked spec defines the interface, the fill rule and the acceptance criteria; do not widen scope.
- With `protective` absent the bar loop takes exactly today's path. Existing fixtures, gate tests and `BACKEND_REV` stay untouched.
- `DecisionStep` and the exit rules are not edited: the live evaluator shares them.
- No clock, environment or I/O in `q-engine`; the source is the only way prices enter.
- Use the existing repository toolchain and canonical checks without resource-slice wrappers.
- Commit focused changes on the task branch. Never push, merge or change protected branches.

## Ordered implementation

- [ ] 1. Add `crates/q-engine/tests/protective_orders_gate.rs` with the hand-computed cases of spec criteria 1 to 9 and a counting test source. Confirm it fails to compile for the expected reason.
- [ ] 2. Add `crates/q-engine/src/candle/protective.rs`: `ProtectiveColumns`, `IntrabarSource`, `IntrabarPrices`, `RejectedEntries`, the screen and the price walk as pure functions with unit tests for the boundary comparisons (at the level, one step beyond, entry-bar skip).
- [ ] 3. Extend `candle/inputs.rs`, `candle/config.rs` (`CandleError` variant for a source failure) and `candle/run.rs`: validation, the entry check in `open_entry`, the new step, `ExitReason::StopLoss` and `ExitReason::TakeProfit`, `exit_time_us` in the ledger and `rejected` in `CandleRun`. Update the three `CandleInputs` constructors (`run.rs` tests, `candle_engine_gate.rs`, `q-py/src/engine.rs`) to pass `None`.
- [ ] 4. Run `cargo test -p q-engine`; confirm the new gate passes and `candle_engine_gate`, `decision_step_gate` and `exit_rules_gate` pass with unchanged bodies. Commit.
- [ ] 5. Project through `crates/q-py/src/engine.rs`: the three keyword arguments, a source that calls the Python callable and keeps its exception for re-raising, the new result arrays, `exit_reason_text` for codes 14 and 15, and `PROTECTIVE_ORDERS`. Add cases to `tests/test_engine.py` for spec criterion 11.
- [ ] 6. Document the fill rule in `README.md`. Run `make bench-candle-kernel` on `development` and on the branch and record both numbers here. Run `make check`. Commit.

## Review focus

- The screen and the walk use the same comparisons, so a bar the screen rejects can never hold a fill: `<=` and `>=` for stops, strict `>` and `<` for targets, in both places.
- The entry-bar skip applies only to the trade opened on that bar, not to an older trade still open.
- A trade closed by the new step is absent from the positions section D passes to `DecisionStep`, and its rule state is gone on the next bar.
- Capital is updated before section D so a later entry sizes on it.
- No call to the source when `protective` is absent, on warm-up bars, or on bars force-closed in section A.
- A Python exception inside the callable is re-raised as itself, not wrapped.

## Validation and handoff

Run `cargo test -p q-engine --test protective_orders_gate`, then `make check`. Fixtures are new hand-computed cases; existing reference outputs and `BACKEND_REV` remain unchanged.

Record acceptance results, the two benchmark numbers and open follow-ups.
Commit the final changes, then run `./work board set Q-102 in-review -m "<changes; checks and results; follow-ups>"`
from the workspace root. The human owns integration and the release; Q-104 pins the published tag.
