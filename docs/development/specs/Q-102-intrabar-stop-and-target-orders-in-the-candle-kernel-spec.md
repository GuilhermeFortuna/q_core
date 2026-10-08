# Q-102: Intrabar stop and target orders in the candle kernel

**Status:** written spec and plan awaiting human review; status of record is the [Q project board](https://github.com/users/GuilhermeFortuna/projects/2).
**Batch:** 18 — Stop and target orders in research backtests
**Depends on:** none
**Implementation plan:** [Plan](../plans/Q-102-intrabar-stop-and-target-orders-in-the-candle-kernel-plan.md)

## Purpose

Let an entry carry a stop price and a target price that the candle kernel fills inside the bar where they trade, using the bar's own trade prices, and only loading those prices for bars that can contain a fill.

## Current system

`run_candle` fills every entry and exit at a bar's open. Exit rules are evaluated on each closed bar by `DecisionStep` and close at the next bar's open, so a stop level and its fill price differ. That behaviour is shared with the live forward evaluator and stays as it is. The tick kernel (`tick/simulate.rs`) has per-entry stops and targets, but it reads bid and ask for every tick of the stream and cannot run a candle strategy.

## Required behavior

### Inputs

- `CandleInputs` gains two optional members, both absent by default:
  - `protective`: per-bar `stop_price` and `target_price` columns. A value is read only on a bar whose `entry` is not zero and belongs to the entry that bar queues. `NaN` means no level.
  - `intrabar`: an `IntrabarSource`, a trait with one method that returns the trade prices of one bar in the order they traded, each with its wall-clock time in microseconds.
- Levels are absolute prices. The kernel does not know tick sizes, timeframes, sessions or where the prices come from.
- Validation, before any bar is simulated: `protective` requires `intrabar`, `open`, `high` and `low`; both columns have the series length; a level is `NaN` or finite and positive.

### Entry

- A queued entry with levels is checked against its fill price in section C. A long needs `stop < fill` and `target > fill`; a short needs `stop > fill` and `target < fill`. Only the levels that are set are checked.
- An entry that fails the check is not opened. It is reported in a new `rejected` output with the fill bar, side, fill price and both levels. Nothing else changes on that bar.

### Fill rule

A new step runs after section C and before section D on every tradable bar that is not force-closed in section A.

1. **Screen.** For each open trade with a level, compare the bar's range to the level:
   - long: stop when `low <= stop`, target when `high > target`;
   - short: stop when `high >= stop`, target when `low < target`.
   A bar that fails the screen for every open trade does not call the source.
2. **Resolve.** Otherwise the source is called once for that bar and its prices are walked in order. For a trade opened on this bar the first price is its own fill and is skipped.
   - A stop triggers on the first price at or beyond the level and fills at that price. A bar that opens beyond the level therefore fills at its open.
   - A target triggers on the first price strictly beyond the level and fills at the level. A price that only touches the level does not fill.
   - The first trigger in price order closes the trade. One price cannot trigger both.
3. **Close.** The trade closes on this bar with reason `STOP_LOSS` (code 14) or `TAKE_PROFIT` (code 15), its exit-side cost charged at the fill price, and capital updated before section D. The ledger records the triggering price's time in a new `exit_time_us` column (`-1` for every other exit).
4. Prices are the authority. A bar that passes the screen but holds no triggering price leaves the trade open. A source that returns no prices, or fails, ends the run with a `CandleError` naming the bar.

Each trade carries its own levels and a protective fill closes that trade only. Section D sees the trade as closed: its exit rules are not evaluated and the exit book drops its state as for any closed trade. An entry signal on the same bar queues for the next open as usual.

### Unchanged

- With `protective` absent, every output is bitwise identical to today, and no existing fixture, gate test or benchmark input changes.
- `DecisionStep`, the exit rules, sizing, day-trade sections A and E, `force_close_at_end` and the tick kernel are not modified.
- The source is called at most once per bar, in ascending bar order, and never for a bar that fails the screen. Two runs over the same inputs and source produce identical output.

## Interfaces and ownership

- `q-engine` owns the screen, the fill rule and the rejection rule. The host owns where prices come from, which prices count as trades, and time zones.
- `q-engine` exports `ProtectiveColumns`, `IntrabarSource`, `IntrabarPrices`, `RejectedEntries` and the two new `ExitReason` variants.
- `q-py` `run_candle` gains keyword arguments `stop_price`, `target_price` (float64 arrays) and `intrabar` (a callable taking the bar index and returning a `(time_us, price)` pair of int64 and float64 arrays). The result gains `exit_time_us` and the `rejected_*` arrays; existing keys keep their names, dtypes and order. An exception raised by the callable reaches the caller unchanged.
- The Python module exposes `PROTECTIVE_ORDERS = True` so a consumer's startup check can require this release.
- `README.md` documents the fill rule in the `q-engine` section.

## Acceptance criteria

Hand-computed fixtures on small synthetic series; no backend reference export is involved, because the backend has no implementation of this rule.

1. Long with stop 95 and target 110, entry at 100: a bar with prices 101, 96, 95, 94 closes at 95 with `STOP_LOSS`; the mirrored short closes at its stop.
2. A bar whose range contains both levels resolves by price order: 100, 111, 94 closes at 110 with `TAKE_PROFIT`; 100, 94, 111 closes at 94 with `STOP_LOSS`.
3. A price equal to the target does not fill; the next higher price fills at the target.
4. A bar that opens beyond the stop fills at its open price.
5. On the entry bar the first price is skipped: an entry at 100 with stop 99 and prices 100, 99.5, 99 closes at 99.
6. A long entry whose stop is at or above its fill price is not opened and appears in `rejected` with its bar, side, fill price and levels.
7. A counting source is called only for bars that pass the screen and once per bar; an empty or failing source yields a `CandleError` naming the bar.
8. A screen pass with no triggering price leaves the trade open.
9. A day-trade run closes a protected trade at the session close when neither level trades, and never calls the source on a force-closed bar.
10. Runs without `protective` reproduce every existing candle fixture and gate test with unchanged bodies; a double run with `protective` is identical.
11. The Python binding returns the same ledger as the Rust test for criteria 1, 2 and 6, and re-raises a callable's exception.
12. `make check` passes. `make bench-candle-kernel` is run before and after and both numbers are reported.

## Implementation boundary

This issue authorizes only its listed deliverable after written-plan approval and
`./work start Q-102 --agent <agent> --worktree`.
No stop or limit entry orders, no change to a level after entry, no bid or ask fill model, no bar-only fallback when prices are missing.
The live forward evaluator keeps deciding on closed bars; placing broker-side protective orders is a later batch.
No contract change, no Qt binding change and no unrelated refactoring.
