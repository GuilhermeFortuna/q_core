# Q-029: Tick kernel and bar aggregation

**Status:** authoritative in the [Q project board](https://github.com/users/GuilhermeFortuna/projects/2)  
**Project direction:** [`q_contracts/docs/system-architecture.md` §2, §3.3, §5, §9 invariant 1, §10 Phase 2](https://github.com/GuilhermeFortuna/q_contracts/blob/f2273a88e52c5b9a8ad5ac9d7eb27f8069cf643d/docs/system-architecture.md#2-assessment-of-the-current-system)  
**Depends on:** Q-021  
**Implementation plan:** [`../plans/Q-029-tick-kernel-and-bar-aggregation-plan.md`](../plans/Q-029-tick-kernel-and-bar-aggregation-plan.md)

## Purpose

Tick backtests in `q_backend` already have the right shape: a strategy emits
aligned arrays and a compiled kernel walks the ticks. That kernel is a `numba`
function, though, so the tick engine is the one simulation that is neither in
`q_core` nor covered by its parity gate, and architecture §2 moves it to Rust
with the candle loop so that there is one kernel family. Two neighbouring
semantics run as Python over ticks: splitting a stream into trading days, and
aggregating ticks into the bars the tick backtest chart shows, which loops once
per bar in Python. This task implements the single-position tick simulation, the
day split and the tick-to-bar aggregation in `q-engine`, and proves each equal to
today's implementation value for value, before `q_backend` swaps them in
(Q-030).

## Requirements

### Tick simulation

- A simulation takes aligned tick columns (bid, ask), the strategy's direction
  per tick, stop-loss and take-profit distances in price points per tick (missing
  meaning none), initial capital, point value, and a sizing model. It holds at
  most one position at a time.
- While flat, a non-zero direction opens a position if sizing gives a positive
  quantity: long at the ask, short at the bid. The stop and target distances of
  the entry tick fix the stop and target prices for the life of the position.
- While in a position, each tick fills at the bid for a long and the ask for a
  short, and exits with the first of: stop loss reached, take profit reached, an
  opposite direction. Reaching a level includes touching it. The tick that closes
  a position never opens one.
- A position still open after the last tick closes there with reason
  `END_OF_DAY`. Exit reasons keep today's codes: stop loss 1, take profit 2,
  signal 3, end of day 4.
- Profit and loss is the price difference times quantity times point value, and
  capital compounds by it when a position closes. There are no transaction costs,
  as today.
- The two sizing models the tick engine supports are reproduced exactly: fixed
  quantity uses the configured quantity as given, without flooring; fixed safety
  margin truncates capital over margin toward zero, applies the maximum contracts
  when one is set, and applies the minimum-contracts rule as today. A zero maximum
  means no maximum.
- The simulation returns its trades as columns (entry tick, exit tick, entry
  price, exit price, direction, quantity, exit reason) and the final capital,
  computed with the same operations in the same order as today, so results are
  equal bit for bit.

### Day split

- A tick stream is split into trading days as today: consecutive runs of ticks
  whose millisecond timestamps fall on the same UTC day. The split returns the
  start and end of each run, so the caller can compute signals per day and
  simulate each day with fresh capital.
- An empty stream has no days. Ticks are never sorted or dropped.

### Bar aggregation

- Ticks aggregate into display bars as today. The bar price series is the last
  price when any tick in the stream has a positive last price, and the bid-ask
  midpoint otherwise. Consecutive ticks whose timestamps fall in the same bar
  interval form one bar, whose open time is the interval start.
- Each bar has the open, high, low and close of its prices, its summed volume
  truncated to a whole number, and the first and one-past-last tick it covers.
  Missing prices propagate into high and low exactly as today, and the volume sum
  is bit-identical to today's before truncation.
- For an indicator series aligned with the ticks, the value of each bar is the
  value at its last tick, reported as missing where that value is missing.
- The bar interval grows as today when a stream's span would produce more than
  50,000 bars: it doubles until the count fits or it exceeds one day.
- Timestamp formatting, display timeframe names and validation of a requested
  timeframe are presentation and stay with the caller.

### From Python

- The wheel runs the simulation, the day split and the aggregation from numpy
  arrays and returns numpy arrays, with no Python object per tick or per bar.
- Arrays of the wrong dtype or length raise a Python exception naming the
  argument and are never cast silently. A sizing model the tick engine does not
  support is rejected with an error naming it. No input aborts the interpreter.

### Parity and determinism

- Parity is proven by reference fixtures exported from `q_backend` through the
  Q-021 exporter at the existing reference pin, checked by the Q-021 gate under
  the exact policy: simulation fixtures from the compiled tick kernel, day-split
  fixtures from the tick engine's splitter, and bar fixtures from the tick chart
  resampler and indicator sampler.
- Simulation scenarios cover: stop loss and take profit both reached on one tick;
  take profit alone; opposite-signal exit with a direction on the exit tick; long
  and short spread losses; end-of-stream close; missing stop and target;
  fractional fixed quantity; safety-margin compounding with minimum and maximum
  contracts, including capital that turns negative; and a full synthetic stream
  with real strategy directions.
- Aggregation scenarios cover: last-price bars; midpoint bars when no last price
  is positive; a missing price inside a bar; fractional volumes; interval
  doubling; indicator sampling with missing values; and ticks out of time order,
  which produce repeated bar intervals today.
- Day-split scenarios cover a multi-day stream, a single day and an empty stream.
- The same inputs run twice in one process give bit-identical outputs.

### Preserved guarantees

- The Q-022 and Q-025 gates, and any `q-engine` gate merged before this task, keep
  passing unchanged. `q-engine` stays free of unsafe code, input and output,
  clocks and environment access, with its dependency direction unchanged.
- The wheel's existing functions are unchanged and it still builds without Qt.

## Constraints and non-goals

- **No change to `q_backend`.** Tick backtests and the tick chart move to these
  functions in Q-030.
- **No tick strategies in Rust.** Strategies compute directions and stop and
  target distances with vectorised numpy, as the tick strategy contract requires,
  and that stays in Python.
- **No sharing with the candle kernel.** The tick engine's fixed quantity is not
  floored, its safety margin truncates instead of flooring, and it charges no
  costs. Unifying either with Q-027's sizing would change results of one engine.
- **No fixes to quirks found along the way.** The re-entry block flag has no
  effect, tick days are UTC while candle days are local, fixed quantity is not
  floored, volatility-targeted sizing is not supported, and the price series
  choice between last and midpoint is made once for the whole stream. All are
  recorded in the backend's findings and reproduced.
- **No live forming-bar aggregation.** The live market publisher receives bars
  from MetaTrader 5, not ticks, so no consumer needs it. Aggregating live ticks is
  a later decision with its own consumer.
- **No Q-025 frame output.** Out-of-order ticks produce repeated bar times today,
  which a frame rejects. A caller with sorted ticks can build a frame from the
  returned columns.
- **No multi-position or intrabar-candle simulation, costs, or slippage.**
- **No move of the `q_backend` reference pin.**

## Acceptance criteria

### Agent-verifiable

1. Unit tests state the simulation rules as cases: stop checked before target,
   touching a level exits, the exit tick never opens a position, entry-tick
   distances fix the levels, missing distances disable them, end-of-stream close
   uses reason 4, fixed quantity is not floored, safety-margin truncation toward
   zero with a zero maximum meaning none.
2. Unit tests state the day split and aggregation rules as cases: UTC day runs,
   empty input, last-price versus midpoint choice over the whole stream, missing
   prices propagating into high and low, volume truncation, interval doubling past
   one day, and indicator values sampled at each bar's last tick.
3. Tick-kernel, day-split and tick-bar reference fixtures exported from
   `q_backend` at the existing pin pass the Q-021 gate under the exact policy for
   every scenario listed under Parity. Negative controls each fail: one price
   moved by one ULP; one exit tick moved by one; one final capital moved by one
   ULP; one bar volume changed by one; a checksum mismatch; and an unaccounted
   fixture file.
4. The Q-021 double-run determinism check passes for every scenario.
5. Through the installed wheel, the simulation, day split and aggregation return
   arrays equal to one fixture scenario each; a float32 bid array, a length
   mismatch and an unsupported sizing model each raise an exception naming the
   argument or model.
6. Existing gates pass unchanged and `make parity-isolation` passes.
7. The full validation suite passes: `make check`.

### Human-verifiable

1. The tick fixtures are regenerated from the pinned backend in its full
   environment and show no diff. The regeneration time is reported.
   Command: `cd q_core && time make fixtures-backend-check`
2. Simulation throughput is compared with the compiled Python kernel at the pin
   on a 5,000,000-tick synthetic stream, five runs each, reporting individual and
   median wall times for both, plus the Python kernel's first-call time including
   compilation.
   Command: `cd q_core && make bench-tick-kernel`
