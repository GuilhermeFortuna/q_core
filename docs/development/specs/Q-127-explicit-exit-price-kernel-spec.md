# Q-127: Explicit exit price in the candle kernel

**Status:** written spec and plan awaiting human review; status of record is the [Q project board](https://github.com/users/GuilhermeFortuna/projects/2).
**Batch:** 32 — Explicit exit prices in research strategies
**Depends on:** none
**Implementation plan:** [Plan](../plans/Q-127-explicit-exit-price-kernel-plan.md)

## Purpose

A closed-bar runtime close fills only at the next bar's open, and the only intrabar close is a screen/tick callback pair that the caller must drive. Research strategies need to close at a price they computed before the candle opened, provided the market actually traded there during the candle. After this task:
- a closed-bar decision can carry an `exit_price`; the kernel fills the close at the first trade of the candle that reaches it;
- the screen and tick callbacks are removed, since the research layer (Q-128) no longer uses them.

## Current system

- Closed-bar decisions come from `strategy_callback` (`crates/q-engine/src/candle/run.rs`, `decide`), which returns `exit_long`/`exit_short`. A close fills at the next bar's open (`fill_price`).
- `intrabar_fills` applies stops, targets, breakeven and resting orders from the candle's trade prices, served by `IntrabarSource`.
- `exit_screen_callback` and `exit_tick_callback` (`crates/q-py/src/engine.rs`, `run_candle_with_callbacks`) let a caller screen candles and decide per trade. They are documented in `README.md`.
- Amend and resting orders already require an intrabar source, `open`, `high` and `low`; the same requirement applies here.

## Behaviour

### Exit price in the decision

- A closed-bar decision may carry `exit_price` (finite float). A missing or NaN value means no price.
- `exit_price` applies to a close in the same decision (`exit_long` for the long position, `exit_short` for the short one).
- Validation, raised as `CandleError` naming the bar and `exit_price`:
  - the value must be finite and positive;
  - it must lie within the bar's `[low, high]`;
  - the intrabar source, `open`, `high` and `low` must be present, as for amend.

### Fill

- Stops, targets and breakeven fills of the bar are applied first, as today. A priced close is considered only if the position is still open afterwards.
- The priced close scans the bar's trades in stored order, starting at the later of the bar's open and the position's fill time.
- The close fills at `exit_price` itself, for a long or a short, timed at the first scanned trade that reaches it from the side the scan started on: at or above it when the first scanned trade is below it, otherwise at or below it. The exit reason is the one a closed-bar close records today.
- Amended after Q-127 shipped: the first version filled a long only at the first trade at or below `exit_price` (a short at or above), at that trade's price. A close above a long's scan start then filled at the first trade, near the open, instead of the requested price.
- If no trade of the bar reaches the price, the close does nothing and the position stays open. It does not fall back to the next open.
- A close without `exit_price` is unchanged.

### Removal

- `exit_screen_callback` and `exit_tick_callback` are removed from the Python API, `run_candle_with_callbacks`, the intrabar tick replay they drive, and `README.md`.

### Ordering choice

The decision is taken after the bar's intrabar fills, so a priced close never pre-empts a stop that trades earlier in the same bar. This is deliberate: it keeps the strategy's view of its positions unchanged.

## Focused acceptance

1. A close with `exit_price = P` on a bar whose trades reach P fills at P, at the time of the first trade reaching P from the side the scan started on, for a long or a short and whichever side of the scan start P lies on.
2. `exit_price` outside the bar's `[low, high]` raises `CandleError` naming the bar and `exit_price`.
3. A non-finite or non-positive `exit_price` raises.
4. A stop that fills earlier in the bar closes the position; the priced close then does nothing.
5. If no trade reaches the price, the position stays open and later bars are unaffected.
6. A priced close on the bar where the position was filled scans from the fill, not from the bar's open.
7. A priced close without an intrabar source raises.
8. Passing `exit_screen_callback` or `exit_tick_callback` is rejected, since the parameters no longer exist.
9. Existing kernel tests pass after the removed-callback tests are deleted, and the research parity tests do not change.

## Delivery boundary

Out of scope:
- the research hooks, `TradeOrder` and `TickData` (Q-128);
- changing closes without a price, which still fill at the next open;
- limit-style exits resting across bars.

The task ends with a release tag under `RELEASING.md`, which Q-128 pins.
