# Q-118: Resting orders and stop amendments in the candle kernel

**Status:** written spec and plan awaiting human review; status of record is the [Q project board](https://github.com/users/GuilhermeFortuna/projects/2).
**Batch:** 28 — Fast tick-backed backtests
**Depends on:** none
**Implementation plan:** [Plan](../plans/Q-118-resting-orders-and-stop-amendments-in-the-candle-kernel-plan.md)

## Purpose

Let a strategy that decides on closed bars place stop and limit entry orders and move the stop and target of open trades, with every fill resolved in Rust from the bar's trade prices. Strategies currently get that behaviour by calling a Python hook on every tick, which costs a full indicator rebuild per call (Batch 28, finding 1). After this task they can express it as orders, and the kernel matches them on the ticks.

## Current system

- An entry is decided at the close of bar `i`. It fills at the open of bar `i+1` (Q-027), or on bar `i` at a price inside that bar's range (Q-105).
- Its stop and target are fixed when the trade opens (Q-102). Nothing changes them later.
- `intrabar_fills` scans a bar's trade prices only when the bar's range reaches an open trade's level, or when the custom exit screen asks for it (Q-104).
- The strategy callback returns a 4-, 6- or 7-value tuple.
- Position snapshots carry ordinal, side, entry bar, entry price and quantity.
- An entry whose levels are on the wrong side of its fill is rejected before sizing and the position cap are applied. Q-104 recorded this as a known deviation.

## Required behavior

### Resting entry orders

- A runtime decision that queues an entry may carry a resting order instead of a market or priced entry: kind `stop` or `limit`, a positive finite order price, and `expire_bars`. `expire_bars` is a positive integer, or none for "until filled, replaced or cancelled".
- **Placement.** The order is placed in section D of the deciding bar, after the day-trade window and the other entry gates. A gated decision places nothing. It is working from the first price of the next bar.
- **One working order.** There is at most one working entry order per run. A new entry decision of any kind replaces it, and a `cancel_entry` decision removes it.
- **Expiry.** An order with `expire_bars = k` placed on bar `i` can fill on bars `i+1` to `i+k`, and then lapses. In day-trade mode a working order also lapses at the force-close bar and after the last bar of a day; it never carries into the next session.
- **When ticks are read.** A bar's trade prices are read when its range can reach the order:
  - buy stop: `high >= price`
  - buy limit: `low < price`
  - sell stop: `low <= price`
  - sell limit: `high > price`

  Resting orders require `intrabar`, `open`, `high` and `low`, as levels already do. Without them the run fails with `CandleError::InvalidConfig`.
- **Fill rules,** consistent with Q-102:
  - A stop order fills at the first trade price at or beyond its price, so a gap through it fills at the gap price.
  - A limit order fills at its price on the first trade strictly beyond it.
- **On fill.** The trade is sized with the capital at that moment. The position cap is applied first: an order the cap reduces to zero is consumed without a trade and is not reported as rejected. Then the wrong-side rule is checked against the actual fill price; a failing order is consumed and reported in `rejected` with that fill price.
- **The new trade's levels** become active from the next price in the same scan. A trade never exits on the price that opened it.

### Amending open trades

- A runtime decision may carry `amend_stop` and `amend_target`. `NaN` keeps the current level. Any other value must be finite and positive, and replaces the level.
- An amendment applies to the trades in the snapshot the callback received. It never applies to an entry decided in the same call.
- The new levels are in force from the first price of the next bar, under the existing protective rules: range screen, gap fill at the traded price, and a target filled at its level when traded strictly through.
- A level that is already beyond the next open fills at that open, by the gap rule. No error is raised.
- Amendments never remove a level, and never change exit rules or breakeven state.

### One scan per bar

Within a bar, every event is resolved in a single walk over its trade prices: protective exits, the custom tick exit (Q-104), then a resting entry fill. If several happen on the same price, they apply in that order. A protective exit and a reversing entry on the same price therefore close the old trade, then open the new one.

### Entry check order (fixes the Q-104 deviation)

For every entry kind — open fill, priced and resting — sizing and the position cap run before the wrong-side rule. An entry the cap skips is never reported in `rejected`.

### Unchanged

- Without resting orders or amendments, every output is bitwise identical to today, except entries that are both capped and wrong-side: they are no longer reported in `rejected`. Existing fixtures and `BACKEND_REV` do not change.
- `DecisionStep`, the exit rules, sizing and the tick kernel are not modified. The live evaluator shares them.

## Interfaces and ownership

- **Ownership.** `q-engine` owns the working order, matching, expiry and amendments. The host owns the decisions.
- **Callback form.** `BarSignals` gains optional order and amendment fields with a default of none. The `q-py` strategy callback may return a `dict` with the keys `entry`, `exit_long`, `exit_short`, `strength`, `stop_price`, `target_price`, `entry_price`, `entry_kind` (`"market"` or omitted, `"stop"`, `"limit"`), `expire_bars`, `cancel_entry`, `amend_stop` and `amend_target`. Missing keys take today's defaults. A `dict` that is not a valid decision raises `ValueError` naming the bar and the key. The existing tuple forms are unchanged.
- **Snapshots.** `run_candle(..., snapshot_levels=False)` keeps today's 5-field position tuples. With `True`, each tuple gains the trade's current stop and target (`NaN` where unset), and the Rust `PositionSnapshot` gains both.
- **Ledger.** The ledger and the binding's result gain:
  - `entry_kind`: `0` open fill, `1` priced, `2` stop order, `3` limit order;
  - `entry_time_us`: the trade price's time for a resting fill, `-1` otherwise;
  - `final_stop_price` and `final_target_price`: the levels in force when the trade closed.

  Existing keys keep their names, dtypes and order.
- **Capability flag.** `q_core.engine.RESTING_ORDERS` is `True`.
- **Documentation.** `README.md` documents the order, expiry, amendment and same-price rules in the `q-engine` section.

## Acceptance criteria

The fixtures are small synthetic series computed by hand.

1. A buy stop at 101, placed on bar 0, fills on bar 1 at the first price `>= 101`. When bar 1 opens at 102, it fills at 102.
2. A buy limit at 99 fills at 99 only after a price below 99. A price equal to 99 does not fill it.
3. The sell stop and sell limit mirror criteria 1 and 2.
4. An order with `expire_bars = 2` fills on bar 2 and not on bar 3. In day-trade mode a working order lapses at the session's last bar and never fills the next day.
5. A new entry decision replaces a working order, and `cancel_entry` removes one; neither then fills.
6. A long opened at 101 by a stop order, with a stop at 100, on a bar trading 100.5, 101, 100.5, 100: it opens on the second price and is stopped on the fourth, both on the same bar.
7. A stop amended from 95 to 98 on bar 2's close is not triggered by bar 2's prices. Bar 3 trading 98 closes the trade at 98 with `STOP_LOSS`, and `final_stop_price` is 98.
8. An amendment above the next open, for a long, fills at that open.
9. A protective exit and a reversing resting entry on the same price close, then open, in that order.
10. A bar whose range reaches neither an order nor a level does not call a counting source.
11. An order that is both capped and wrong-side opens nothing and is not in `rejected`. An order that is wrong-side and not capped is in `rejected`, with its fill price.
12. A run without the new fields reproduces every existing fixture and gate test unchanged. A double run is identical.
13. The Python binding returns the same ledgers as the Rust tests for criteria 1, 6 and 7, and rejects an invalid `dict` naming the key. `snapshot_levels=True` tuples carry the amended stop.
14. `make check` passes.

## Implementation boundary

This issue authorizes only its listed deliverable after written-plan approval and
`./work start Q-118 --agent <agent> --worktree`.

Out of scope:
- stop-limit and one-cancels-other entry pairs;
- more than one working entry order, and removing a level;
- resting orders through static columns;
- a bid or ask fill model;
- any change to the live evaluator, contracts or the Qt binding.
