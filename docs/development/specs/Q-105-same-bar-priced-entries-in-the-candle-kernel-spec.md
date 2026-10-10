# Q-105: Same-bar priced entries in the candle kernel

**Status:** written spec and plan awaiting human review; status of record is the [Q project board](https://github.com/users/GuilhermeFortuna/projects/2).
**Batch:** 19 — Same-bar priced entries
**Depends on:** Q-102
**Implementation plan:** [Plan](../plans/Q-105-same-bar-priced-entries-in-the-candle-kernel-plan.md)

## Purpose

Let an entry name its own fill price and fill on the bar that decided it, so a strategy can buy at the previous bar's high when the current bar trades through it.

## Current system

`run_candle` queues every entry from the closed bar `i` and fills it in section C of bar `i+1` at that bar's open. Q-102 adds per-entry stop and target levels resolved inside a bar from trade prices, but an entry itself can only fill at an open.

## Required behavior

### Inputs

- `ProtectiveColumns` gains an optional `entry_price` column. `NaN` means no price. A value is read only on a bar whose decision queues an entry.
- Validation, before any bar is simulated: `entry_price` requires `open`, `high` and `low`; it has the series length; a value is `NaN` or finite and positive. An entry price together with stop or target levels also requires `intrabar`.

### Entry

- A decision that queues an entry with a price does not queue it for the next bar. In section D of the same bar, the kernel opens the trade at exactly that price, sized with current capital and charged its entry cost at that price.
- The price must satisfy `low <= price <= high` of the deciding bar. Otherwise the run ends with a `CandleError` naming the bar, side, price, low and high.
- The wrong-side rule of Q-102 applies against the entry price: a long needs `stop < price` and `target > price`; a short the mirror. A failing entry is not opened and is reported in `rejected` with the entry price as its fill price.
- Day-trade windows are unchanged: no entry is decided on a force-close bar, the last bar of a day or outside the entry window, so none can fill there.
- An entry with no price behaves exactly as today.

### Levels on the entry bar

- The entry's own stop and target resolve from the bar's trade prices that follow the entry's touch. The touch is the first price that is at or beyond the entry price on the side the bar opened from: the open itself if it equals the price; otherwise the first price that crosses it. Prices before the touch are ignored; the touching price itself is the entry and is skipped, as Q-102 skips the first price of an open-fill.
- A bar whose range cannot reach a level after the touch does not call the source.

### Unchanged

- With `entry_price` absent every output is bitwise identical to today; existing fixtures, gates and `BACKEND_REV` do not change.
- `DecisionStep`, the exit rules, sizing and the tick kernel are not modified.

## Interfaces and ownership

- `q-engine` owns the range rule, the fill and the touch search. The host owns prices and time.
- `q-py` `run_candle` gains the keyword `entry_price` (float64 array); the callback levels tuple carries it for runtime strategies. Existing result keys keep their names, dtypes and order.
- `README.md` documents the rule in the `q-engine` section.

## Acceptance criteria

Hand-computed fixtures on small synthetic series.

1. A long with price equal to the previous high, on a bar whose high exceeds it, opens on that bar at the price; the ledger entry bar is the deciding bar.
2. A price below the bar's open (a pullback) fills at the price when the low reaches it.
3. A price above the bar's high or below its low ends the run with a `CandleError` naming the bar.
4. A long at price 100 with target 101 on a bar that opened at 99 and traded 99, 100, 99.5, 101.5: the touch is the second price (100), so the trade opens at 100 and closes at 101 on the same bar; the first price is before the touch and is ignored.
5. A long whose stop is at or above its price is rejected with its bar, side and price.
6. A decided entry with a price is not queued: the next bar does not open a second trade.
7. A counting source is not called on a bar whose range cannot reach a level after the touch.
8. A run without `entry_price` reproduces every existing fixture and gate test unchanged; a double run is identical.
9. The Python binding returns the same ledger as the Rust test for criteria 1 and 4 and raises for criterion 3.
10. `make check` passes.

## Implementation boundary

This issue authorizes only its listed deliverable after written-plan approval and
`./work start Q-105 --agent <agent> --worktree`.
No order lifetime beyond the deciding bar, no resting orders, no change to a level after entry, no bid or ask fill model, no live-evaluator change, no contract change and no Qt binding change.
