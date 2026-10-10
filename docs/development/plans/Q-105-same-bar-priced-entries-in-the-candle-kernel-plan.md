# Q-105 implementation plan: Same-bar priced entries in the candle kernel

> **For implementation agents:** Read the linked spec and repository instructions.
> Use superpowers:executing-plans when that skill is available. Start only through
> `./work start Q-105 --agent <agent> --worktree` after written-plan approval.
> Implement this task natively; delegation requires separate authorization.

**Goal:** An entry decided on bar `i` can carry a price and fill on bar `i` at that price when the bar's range contains it.
**Architecture:** `ProtectiveColumns` gains `entry_price`; section D of `run.rs` opens a priced entry immediately and resolves its levels from the trades after the touch; `q-py` projects the column.
**Spec:** [Specification](../specs/Q-105-same-bar-priced-entries-in-the-candle-kernel-spec.md)
**Status:** written plan awaiting human review.

## Global constraints

- The linked spec defines the interface, rule and acceptance criteria; do not widen scope.
- With `entry_price` absent the bar loop takes exactly today's path. Existing fixtures, gates and `BACKEND_REV` stay untouched.
- `DecisionStep` and the exit rules are not edited: the live evaluator shares them.
- No clock, environment or I/O in `q-engine`.
- Use the repository toolchain and canonical checks without resource-slice wrappers.
- Commit focused changes on the task branch. Never push, merge or change protected branches.

## Ordered implementation

- [x] 1. Add `crates/q-engine/tests/priced_entries_gate.rs` with criteria 1 to 7 and a counting source. Confirm it fails for the expected reason.
- [x] 2. Extend `candle/protective.rs`: `entry_price` in `ProtectiveColumns`, validation, the range check and a pure `touch_index(prices, price, open)` with unit tests at the boundaries (equal to open, equal to high, one step beyond).
- [x] 3. In `candle/run.rs` section D: when the decision has an entry and a price, check the range, apply `Levels::rejects` against the price, call `open_entry` with the price as fill, clear `decided.entry`, then resolve its levels over the prices after the touch. Update the `CandleInputs` constructors to pass `None`.
- [x] 4. Run `cargo test -p q-engine`; the new gate passes and `candle_engine_gate`, `decision_step_gate`, `exit_rules_gate` and `protective_orders_gate` pass with unchanged bodies. Commit.
- [x] 5. Project through `crates/q-py/src/engine.rs`: the `entry_price` keyword, the extended callback tuple and the range error. Add cases to `tests/test_engine.py` for criterion 9.
- [x] 6. Document the rule in `README.md`. Run `make check`. Commit.

## Review focus

- A priced entry opens exactly once: it is cleared from `decided` and never reaches `queued`.
- Section B's queued close and section C's queued entry of the *previous* decision still run before this bar's decision.
- The touch search and the range check use the same comparisons, so a bar the check accepts always has a touch.
- Capital is updated before section D, so the priced entry sizes on it.
- Exit-rule state for the new trade starts on the next bar, as for any opened trade.
- No source call when `entry_price` is absent or on bars where the decision queues no entry.

## Validation and handoff

Run `cargo test -p q-engine --test priced_entries_gate`, then `make check`. Record results and follow-ups,
commit, then run `./work board set Q-105 in-review -m "<changes; checks and results; follow-ups>"` from the
workspace root. The human owns integration and the release; Q-106 pins the published tag.
