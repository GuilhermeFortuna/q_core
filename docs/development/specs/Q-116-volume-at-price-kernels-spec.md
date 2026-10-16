# Q-116: Volume-at-price kernels

**Status:** written spec and plan awaiting human review; status of record is the [Q project board](https://github.com/users/GuilhermeFortuna/projects/2).
**Batch:** 27 — Volume at price
**Depends on:** none
**Implementation plan:** [Plan](../plans/Q-116-volume-at-price-kernels-plan.md)

## Purpose

Add the kernels that turn a trade tape into volume at price: how much volume traded at each price level inside a time segment, split by aggressor side, with the segment's point of control and value area. A second kernel reports those levels as they develop, at caller-chosen checkpoints, so a backtest can read them without lookahead.

## Current system

- `q_indicators::volume` (Q-081) classifies trades by aggressor side (`AggressorSide::from_flags`, `BUY_FLAG`, `SELL_FLAG`) and accumulates buy, sell and unknown volume per bar and session for `q_terminal`. It keeps no per-price state and has no Python binding.
- `quotes.rs` (Q-111) shows the batch convention for tape input: columns in source order, caller-owned session keys and `[open, close)` intervals, results as columns.
- Nothing computes volume per price level.

## Required behavior

### Inputs and conventions

- Tape columns: `time_msc` (int64), `price` (the trade's last price), `volume` and `flags` (int64), equal lengths, in source order with non-decreasing times. Equal times keep their order.
- A tick is a **trade** when `flags` has `BUY_FLAG` or `SELL_FLAG`; its side comes from `AggressorSide::from_flags`, so a tick with both bits is `Unknown`. Other ticks are ignored. This is the rule `q_backend`'s tick store already uses for `real_volume`.
- A trade with a non-finite or non-positive price or volume is **skipped** and counted in its segment's `skipped_trades`. Volume is summed as given; the kernel interprets no unit.
- **Price rows.** With tick size `t` (finite, `> 0`) and `row_ticks` `m` (integer `>= 1`): the trade's tick index is `k = round_half_even(price / t)`; the trade is **off grid** when `abs(price / t - k) > 1e-6` and is counted in `off_grid_trades` but still binned at `k`. Its row is `r = floor(k / m)` and the row price is `r * m * t`, the lowest price the row covers. Rows are identified by `r`, never by comparing floats.
- A finite positive price whose rounded tick index cannot be represented as int64 is rejected with an `IndicatorError` naming `price`. Range-centre comparisons use integer arithmetic wide enough for any representable row.
- Value-area share `v` is finite and in `(0, 1]`.
- Sums run in source order. No parallelism, fused multiply-add or reordered sums. NaN is the only missing value.

### Point of control and value area

For a set of rows with totals (buy + sell + unknown):

- The **point of control (POC)** is the row with the largest total. Ties go to the row nearest the centre of the traded range (`(lowest row + highest row) / 2`), then to the lower row.
- The **value area** starts as the POC row. While its volume is below `v × total`, compare the next row above and the next row below inside the traded range, treating a row without trades as `0`. Add the larger one; when they are equal add both; when one side is exhausted add the other. `VAH` and `VAL` are the row prices of the highest and lowest included rows.
- A segment without trades has NaN POC, VAH and VAL.

### `volume_at_price`

`volume_at_price(time_msc, price, volume, flags, segment_open_msc, segment_close_msc, tick_size, row_ticks, value_area)`

- Segments are `[open, close)`, ordered and non-overlapping; a tick at a segment's close belongs to the next segment. Ticks outside every segment are ignored.
- Returns two tables of columns:
  - **rows**, one entry per segment and row with trades, ordered by segment then row price: `segment` (int64 index), `price`, `buy`, `sell`, `unknown`, `total`, `delta` (buy − sell) and `trades` (int64).
  - **segments**, one entry per segment: `total`, `buy`, `sell`, `unknown`, `trades`, `poc`, `vah`, `val`, `skipped_trades` and `off_grid_trades`.
- The segment kind (session, bar, range) is the caller's choice; the kernel reads no clock or calendar.

### `volume_at_price_levels`

`volume_at_price_levels(time_msc, price, volume, flags, session, checkpoint_msc, checkpoint_session, tick_size, row_ticks, value_area)`

- `session` (int64 per tick) and `checkpoint_session` (int64 per checkpoint) are caller keys; checkpoints are non-decreasing in time.
- For each checkpoint it returns `poc`, `vah`, `val` and `total` of the profile built from the trades of `checkpoint_session` with time strictly before `checkpoint_msc`. A checkpoint whose session has no such trade gives NaN levels and total `0`.
- One pass over the tape; the profile resets when the session changes. The value area is recomputed at each checkpoint from the current rows.

### Determinism and rejection

- Both kernels are bit-identical on a double run and prefix-causal: appending ticks after a checkpoint, or segments after a segment, does not change earlier output.
- Unequal column lengths, decreasing tick or checkpoint times, unordered or overlapping segments, an empty segment interval, and out-of-domain `tick_size`, `row_ticks` or `value_area` return an `IndicatorError` naming the function, parameter and value. Nothing panics.

## Interfaces and ownership

- `q-indicators` owns every formula in a new `vap.rs` module that reuses `AggressorSide`, `BUY_FLAG` and `SELL_FLAG`. `volume.rs`, the existing kernels, fixtures and `BACKEND_REV` do not change.
- `q-py` exposes both kernels in `q_core.indicators` over contiguous numpy arrays (times, flags, sessions as int64; prices and volumes as float64). `volume_at_price` returns a tuple of two dicts of arrays (`rows`, `segments`) keyed by the column names above; `volume_at_price_levels` returns one dict. The binding holds no semantics.
- `README.md` documents the row grid, the trade rule, the POC tie-break and the value-area expansion.

## Acceptance criteria

Hand-computed fixtures on small synthetic tapes with tick size `0.5`.

1. Buys of 10 at 100.0 and 5 at 100.5, a sell of 20 at 100.0 and an unknown (both bits) of 3 at 101.0 give rows 100.0 (buy 10, sell 20, total 30, delta −10), 100.5 (total 5) and 101.0 (unknown 3); segment total 38, POC 100.0.
2. A tick with no side bit, and a quote tick with `last = 0`, add nothing; a side-flagged trade with volume `0` or price NaN is counted in `skipped_trades` only.
3. With `row_ticks = 2`, trades at 100.0 and 100.5 share row 100.0, and 101.0 starts the next row. A price of 100.2 is off grid, counted once and binned at 100.0.
4. POC tie-break: totals `[10, 2, 10, 2, 2]` choose the third row (nearer the centre); totals `[10, 1, 10]` choose the first row (equally near, lower).
5. Value area at `0.7` over totals `[5, 10, 40, 10, 30, 5]` (ascending prices) expands from the 40 row: compares 10 and 10, adds both (60), then compares 5 and 30, adds 30 (90 ≥ 70); VAL and VAH are the second and fifth rows. A gap row inside the range counts as `0`. `value_area = 1` covers the whole traded range.
6. Segment boundaries: a tick at a segment's close is counted in the next segment; ticks outside all segments are ignored; an empty segment has NaN levels.
7. `volume_at_price_levels` at a session's final checkpoint (after its last trade) equals that session's `volume_at_price` POC, VAH, VAL and total; a checkpoint before the first trade is NaN; levels reset on a session change; a trade at exactly `checkpoint_msc` is excluded.
8. Both kernels pass the double-run and prefix checks; each rejection in **Determinism and rejection** returns an error and no partial output.
9. The Python binding returns the Rust values for criteria 1 and 7 and raises `ValueError` for an invalid `row_ticks`.
10. The existing reference gate is unchanged and `make check` passes.

## Implementation boundary

This issue authorizes only its listed deliverable after written-plan approval and
`./work start Q-116 --agent <agent> --worktree`.
No streaming or Qt binding, no change to `volume.rs` or any existing kernel or fixture, no contract change, no footprint imbalance or high/low-volume-node detection, no composite or multi-session profiles beyond what segments express, and no I/O, clock or environment access.
