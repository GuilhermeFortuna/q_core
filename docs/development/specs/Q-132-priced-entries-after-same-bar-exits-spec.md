# Q-132: Priced entries after same-bar exits in the candle kernel

**Status:** written spec and plan awaiting human review; status of record is the [Q project board](https://github.com/users/GuilhermeFortuna/projects/2).
**Batch:** 36 — Priced entries after same-bar exits
**Depends on:** none
**Implementation plan:** [Plan](../plans/Q-132-priced-entries-after-same-bar-exits-plan.md)

## Purpose

A priced entry fills on the bar that decided it. The kernel looks for its touch from the bar's first trade,
even when a trade closed earlier in the same bar. A priced entry can therefore open at a price that traded
while the account still held the previous position. The runtime callback runs after the bar's intrabar exits,
so a strategy that only enters when flat sees no position and can take that earlier touch. The entry only
exists because of an exit that happened later in the bar: that is lookahead.

Evidence: an `IND$N` M5 research strategy whose priced entry re-creates an OCO stop bracket
(2026-01-01 to 2026-10-21, 9,090 trades). It used Optuna-chosen levels (stop 0.5 ATR), so many trades exit
on the bar they open.

| Trades | Count | PnL |
|---|---|---|
| Touched before the previous trade's exit on the same bar | 3,765 | +192,312 |
| All others | 5,325 | −6,608 |

When the strategy itself refused to enter on any bar where it had held a position, PnL fell from 185,705 to
47,408, drawdown rose from 7,632 to 19,548 and the profit factor fell from 1.19 to 1.05.

After this task, a priced entry on a bar where a trade closed intrabar fills only from a trade at or after that
exit. The bar's earlier trades no longer count.

## Current system

- Section D of `run_bars` (`crates/q-engine/src/candle/run.rs`) runs after the bar's intrabar fills
  (`intrabar_fills`) and the strategy callback. It applies a priced signal close (`priced_signal_exit`), then a
  priced entry (`priced_entry`, `open_priced_entry`).
- `open_priced_entry` opens the trade at the strategy's price. `resolve_entry_levels` then finds the touch with
  `touch_index` from the bar's first trade, using the bar's open as the reference side. It asks the source only
  when the bar's range reaches one of the entry's levels.
- Intrabar exits record `exit_time_us`. They do not record the index of the scan tick that triggered them.
- `RejectedEntries` lists entries whose stop or target is on the wrong side of the fill price, and carries no
  reason. The Python binding returns it as `rejected_*` arrays (`crates/q-py/src/engine.rs`).
- Priced entries come from the runtime callback's `entry_price` or from the static `entry_price` column. Both
  paths reach `priced_entry`, so they share the bug.

## Behaviour

### The bar's last exit

- The last exit of a bar is the latest scan tick, in stored trade order, at which a trade closed on that bar
  by:
  - a stop, target or breakeven in `intrabar_fills`, including a trade opened by a resting order on the same
    bar;
  - a priced signal close (`priced_signal_exit`).
- Closes at the bar's open (sections A and B) are before every trade of the bar and add no constraint. The
  daily close in section E comes after section D and adds none either.
- A bar with no such exit keeps today's behaviour exactly.

### Priced entry on a bar with a last exit

- The touch search starts at the last exit's scan tick, which is included. Resting fills already share a print
  with exits on that print (`intrabar_fills`), and this keeps the same rule.
- The touch is the first scanned trade that reaches the entry price from the side the scan started on. If the
  first scanned trade is below the price, the touch is the first trade at or above it; otherwise it is the
  first trade at or below it. This is the rule `priced_signal_exit` already uses (Q-127).
- When a touch exists, the trade opens at the strategy's price as today. Its stop and target resolve from the
  trades after that touch, and the trade's `first_tick` is that touch. The kernel asks the source on every such
  bar, even when the range reaches none of the entry's levels.
- When no scanned trade reaches the price, the entry is refused. The trade does not open, and the entry is
  added to `RejectedEntries` with its price as the fill price, its levels, and the new reason below. The run
  does not end with an error.
- The price must still lie in the bar's `[low, high]`, and the refusal for a stop or target on the wrong side
  is unchanged. The range check runs first, then the touch check, then the level check against the price.

### Rejection reasons

- `RejectedEntries` gains `reason: Vec<i8>`:
  - `0`: a stop or target is on the wrong side of the fill price (today's only reason);
  - `1`: a priced entry had no trade at or after the bar's last exit that reached its price.
- The Python binding adds a `rejected_reason` array beside the existing `rejected_*` arrays.
- `README.md` documents the rule and the codes in "Priced entries".

## Focused acceptance

1. A long is stopped at scan tick k. A priced long whose price traded only before k is refused with reason
   `1`, and no trade opens.
2. Same as 1, but the price trades again after k: the entry opens at the strategy's price, with its touch, and
   its levels resolved, from the first reaching trade after k.
3. Same as 1 and 2 with a target exit, and with a short entry after a short exit.
4. A priced signal close fills at tick k. A priced entry from the same decision opens only from tick k
   onward.
5. A trade opened by a resting order and stopped on the same bar sets that bar's last exit.
6. The touch may be the exit's own tick. The new trade's levels start after that tick.
7. A priced entry with no stop or target is still refused when no trade after the exit reaches its price.
8. On a bar with no intrabar exit, every output is identical to the previous release.
9. A wrong-side level refusal reports reason `0`. The binding returns `rejected_reason` with the same length
   as `rejected_bar`.
10. The existing kernel tests pass unchanged, including `tests/priced_entries_gate.rs`,
    `tests/explicit_exit_price.rs` and `tests/resting_orders_gate.rs`.

## Delivery boundary

Out of scope:
- `q_backend` (Q-133): the pin, `rejected_entries` and the research guide;
- how the strategy hook sees positions, and same-bar priced exits;
- a native one-cancels-other resting bracket;
- changes to how a priced entry is priced, including its fill at the strategy's price instead of the first
  trade beyond a stop level.

The task ends with a release tag under `RELEASING.md`, which Q-133 pins.
