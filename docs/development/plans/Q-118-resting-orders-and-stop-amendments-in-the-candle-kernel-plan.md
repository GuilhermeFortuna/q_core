# Q-118 implementation plan: Resting orders and stop amendments in the candle kernel

> **For implementation agents:** Read the linked spec and repository instructions.
> Use superpowers:executing-plans when that skill is available. Start only through
> `./work start Q-118 --agent <agent> --worktree` after written-plan approval.
> Implement this task natively; delegation requires separate authorization.

**Goal:** Runtime decisions can place one stop or limit entry order and amend open trades' levels; the kernel matches both from trade prices in a single scan per bar.
**Architecture:**
- `BarSignals` carries an optional order and an optional amendment.
- `run.rs` owns the working order and applies amendments after section D.
- `intrabar_fills` becomes the one chronological scan for levels, the custom exit and the resting fill.
- `q-py` parses the `dict` callback form and projects the new ledger columns.

**Spec:** [Specification](../specs/Q-118-resting-orders-and-stop-amendments-in-the-candle-kernel-spec.md)
**Status:** written plan awaiting human review.

## Global constraints

- The linked spec defines the interface, rules and acceptance criteria; do not widen scope.
- Without the new fields, the bar loop takes today's path. Existing fixtures, gates and `BACKEND_REV` stay untouched.
- `DecisionStep` and the exit rules are not edited, because the live evaluator shares them. The order and amendment fields pass alongside the decision, not through it.
- No clock, environment or I/O in `q-engine`.
- Use the repository toolchain and canonical checks without resource-slice wrappers.
- Commit focused changes on the task branch. Never push, merge or change protected branches.

## Ordered implementation

- [x] 1. Add `crates/q-engine/tests/resting_orders_gate.rs` covering criteria 1–11 with a counting source. (Written after the implementation, so the expected failure before the change was not observed.)
- [x] 2. In `candle/protective.rs`, add `RestingOrder { side, kind, price, expires_after }` with pure functions for the range screen and the per-price trigger. Unit-test the boundaries: equal to the price, one step beyond, and a gap open.
- [x] 3. In `candle/inputs.rs` and `candle/run.rs`:
  - Extend `BarSignals` with default-none order, cancel and amendment fields.
  - Hold the working order in `run_bars`, with placement in section D after gating, replacement, cancellation and expiry, plus day-trade lapse at force close and at the last bar of a day.
  - Apply amendments to the trades in the callback's snapshot.
- [x] 4. Fold the resting fill into `intrabar_fills`: screen the bar on the union of levels, custom screen and order reach; at each price apply protective exits, then the custom exit, then the order fill. A trade opened by the order skips its own price. Route the fill through `open_entry`.
- [x] 5. Move the wrong-side rejection after sizing and the cap, for open, priced and resting entries, so `open_entry` decides "skipped" versus "rejected".
- [x] 6. Add `entry_kind`, `entry_time_us`, `final_stop_price` and `final_target_price` to `Trade` and `TradeLedger`, and add the levels to `PositionSnapshot`.
- [x] 7. Run `cargo test -p q-engine`. The new gate passes, and `candle_engine_gate`, `candle_callback`, `custom_intrabar_exits`, `protective_orders_gate` and `priced_entries_gate` pass with unchanged bodies, apart from a capped-and-wrong-side case if one exists. Commit.
- [x] 8. In `crates/q-py/src/engine.rs`: parse the `dict` decision with key-named errors, add `snapshot_levels`, the new result keys and `RESTING_ORDERS`. Add `tests/test_engine.py` cases for criterion 13. Commit.
- [x] 9. Document the rules in `README.md`. Run `make check`. Commit.

## Review focus

- A working order fills at most once. It never fills on the bar that placed it, and lapses exactly after `expire_bars` bars or at the session end in day-trade mode.
- Same-price ordering is protective, then custom, then order fill. A new trade's levels start on the following price.
- An amendment is invisible to the bar it was decided on, and never touches a trade opened by the same callback.
- A capped entry is never in `rejected`. A wrong-side entry is in `rejected` with its actual fill price.
- No source call on bars that no level, screen or order can reach.
- The tuple callback forms and the 5-field snapshots are byte-for-byte unchanged.

## Validation and handoff

Run `cargo test -p q-engine --test resting_orders_gate`, then `make check`. Record the results and follow-ups in this plan and commit. Then run `./work board set Q-118 in-review -m "<changes; checks and results; follow-ups>"` from the workspace root. The human owns integration and the release; Q-120 pins the published tag.
