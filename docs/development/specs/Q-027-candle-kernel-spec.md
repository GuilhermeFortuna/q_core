# Q-027: Candle kernel

**Status:** authoritative in the [Q project board](https://github.com/users/GuilhermeFortuna/projects/2)  
**Project direction:** [`q_contracts/docs/system-architecture.md` §2, §3.3, §5, §9 invariants 1 and 5, §10 Phase 2](https://github.com/GuilhermeFortuna/q_contracts/blob/f2273a88e52c5b9a8ad5ac9d7eb27f8069cf643d/docs/system-architecture.md#33-rust-scope-in-execution)  
**Depends on:** Q-024, Q-026  
**Implementation plan:** [`../plans/Q-027-candle-kernel-plan.md`](../plans/Q-027-candle-kernel-plan.md)

## Purpose

The candle backtest loop in `q_backend` walks a pandas frame one row object at a
time: it fills queued orders at each bar's open, sizes entries, caps positions,
charges costs, compounds capital, applies day-trade windows, and asks the exit
rules and the strategy what to queue for the next bar. Architecture §2 names it
the largest throughput cost in research, and every optimizer trial pays it.
After Q-024 a strategy's decision is a set of columns, and after Q-026 the exit
rules are `q_core` state machines, so nothing in the loop needs Python any more.
This task implements that loop in `q-engine` as a kernel over columns, together
with the single-bar decision step the forward evaluator will call, and proves
both equal to today's engine and today's queued-signal reference, trade for
trade and bar for bar, before `q_backend` swaps either in (Q-028, Q-031).

## Requirements

### Inputs

- A run takes columns, not rows: bar open, high, low and close prices; the
  strategy's four decision columns as defined by Q-024 (entry direction, long
  exit, short exit, entry strength); the bar counter when a holding period is
  declared; the indicator columns the enabled exit rules read; and the
  volatility column when volatility-targeted sizing is configured.
- A series without an open price fills at the close, and a series with neither
  fills at zero, exactly as the engine's fallback does today.
- Bar times are given as wall-clock microsecond counts in the clock the caller's
  data uses. The kernel derives calendar days and times of day from them without
  any timezone conversion.
- The kernel accepts bar times in the order it is given, including repeated
  times, because the engine accepts any index today. It never sorts, drops or
  rejects bars for their times.
- A run is configured with: initial capital; point value; transaction costs per
  contract and in basis points, or none; the position-sizing model; the declared
  holding period, if any; the exit-rule parameters; whether day-trade rules apply
  and their entry-window start, entry-window end and force-close times; whether
  open trades are force-closed at the end; and which bars are tradable, so that
  warm-up bars before a trade start produce no action at all.
- Invalid configuration (a non-positive safety margin or target volatility,
  maximum contracts below minimum, mismatched column lengths, a decision column
  outside its contract) is rejected before the loop starts, with an error that
  names the field.

### The bar loop

- For every bar, in order, the kernel reproduces the engine's sequence:
  1. With day-trade rules on, a bar at or after the force-close time closes every
     open trade at that bar's fill price with reason `END_OF_DAY`, discards
     everything queued, and does nothing else on that bar.
  2. Exits queued on the previous bar fill at this bar's fill price. A queued
     close for the strategy's symbol closes every open trade on that symbol, of
     either side, and all of them take the reason of the first queued close.
  3. The entry queued on the previous bar is sized at this bar's fill price with
     the current capital and this bar's volatility, trimmed to the sizer's
     maximum position less the quantity already open on the symbol across both
     sides, skipped when nothing remains, and opened with its entry cost.
  4. The bar is evaluated and its exits and entry are queued for the next bar (see
     the decision step). With day-trade rules on, nothing is queued on a bar at or
     after the force-close time or on the last bar of a calendar day, and no entry
     is queued outside the inclusive entry window.
  5. With day-trade rules on, the last bar of a calendar day closes every open
     trade at that bar's close with reason `END_OF_DAY` and discards everything
     queued.
- The last bar of a calendar day is the last bar of the run, or a bar whose next
  bar has a different calendar date, as the engine decides today.
- When force-close at end is set, trades still open after the last bar close at
  its close with reason `FORCE_CLOSE`. Otherwise they are reported open. Neither
  case is changed here.
- Capital compounds by each closed trade's profit and loss, net of costs, at the
  moment it closes, and later sizing uses the compounded value.
- A trade's cost is its entry-side cost plus its exit-side cost, each computed
  with today's formula, and its profit and loss is price difference times
  quantity times point value, less that cost. With no cost configuration the cost
  is exactly zero.

### The decision step

- One function decides what a closed bar queues, given the bar's position, the
  open trades (each with an identity, side, entry price, and entry bar if the
  entry time is a bar of the series), and the exit-rule state. The bar loop uses
  it for step 4, and it is usable alone for one bar at a time with exit-rule
  state that persists between calls.
- It reproduces today's queued-signal order: exit-rule exits for every open trade
  first, in trade order, each labelled with its rule; then, only if no exit rule
  fired on the strategy's symbol, strategy exits, one per open long trade on a
  long-exit bar and one per open short trade on a short-exit bar; then at most one
  entry with its direction and strength.
- A declared holding period closes a trade when the bars elapsed since its entry
  bar reach the period. A trade with no entry bar in the series is never closed by
  it. While a holding period is declared, the strategy's exit columns are not
  used.
- Strategy and holding-period exits carry no rule reason, and the engine records
  them as `SIGNAL`.

### Position sizing

- The three sizing models the backend offers are reproduced exactly: fixed
  quantity, fixed safety margin with minimum and maximum contracts, and inverse
  volatility with target volatility, point value, minimum and maximum contracts.
  Each optionally scales by entry strength.
- Quantities are floored as today, a sized quantity of zero or less opens nothing,
  a missing or non-positive volatility sizes nothing, and each model's maximum
  position follows today's rule, including the inverse-volatility model's
  fallback to its maximum contracts or no cap.
- Sizing an entry at a given price, capital and volatility is available on its
  own, so the evaluator can size at the bar close with its configured capital.

### Outputs

- A run returns its trades as columns in the order they were opened: entry bar,
  exit bar, side, quantity, entry price, exit price, cost, profit and loss, and exit
  reason. Open trades have no exit bar, exit price, profit and loss or reason.
  Exit reasons are the rule identifiers, `SIGNAL`, `END_OF_DAY` and
  `FORCE_CLOSE`, with the same text the engine records.
- A run also returns, for every bar, what was queued: each queued exit with its
  reason, or no reason for a strategy exit, and the queued entry's direction and
  strength. That decision trace has the shape the backtest-to-live parity
  reference compares.
- Every price, cost and profit and loss is computed with the same operations in
  the same order as the Python engine, so results are equal bit for bit.

### From Python

- The wheel runs a whole series from numpy arrays in one call and returns numpy
  arrays, with no Python object per bar or per trade in either direction.
- The wheel exposes the decision step as an object that keeps exit-rule state
  between calls, identifies open trades by the backend's string ids, and accepts
  exit-rule parameters as the backend's parameter mapping with Python's own
  numeric coercion.
- The wheel reports, for a set of exit-rule parameters, the enabled rules and the
  indicator columns they need; for a sizing model, its maximum position; and for a
  trade in a decision step, its current exit-rule state in the backend's terms. The
  backend then never keeps a second copy of any of these to answer or test them.
- Arrays of the wrong dtype or length raise a Python exception naming the argument
  and are never cast silently. No input aborts the interpreter.

### Parity and determinism

- Engine parity is proven by reference fixtures exported from `q_backend`'s
  backtest engine through the Q-021 exporter at the existing reference pin, and
  checked by the Q-021 gate under the exact policy. Because that pin predates
  Q-024, the exporter supplies decisions through a scripted strategy whose
  decisions are fixture inputs. The loop, not any real strategy, is what these
  fixtures judge, and Q-028's goldens judge real strategies end to end.
- Engine scenarios cover: long and short strategy exits; each exit-rule family
  with its recorded reason; a rule exit and a strategy exit on the same bar;
  simultaneous long and short trades closed together; pyramiding up to and
  trimmed by the position cap; each sizing model including zero-quantity and
  missing-volatility skips and strength scaling; costs of both kinds; a declared
  holding period, including a trade whose strategy exit is ignored; day-trade
  rules with entry-window edges, force-close time and last-bar-of-day closes;
  force-close at end; warm-up bars before a trade start; a close-only series; a
  point value other than one; and repeated bar times.
- Backtest-to-live parity is proven by reference fixtures of the backend's
  queued-signal reference and forward evaluator at the same pin: the queued exits
  and entries for every bar with no open trade and with an open trade held across
  the series, with exit rules enabled, compared with the decision step run one bar
  at a time. This is the port of the backtest-to-live parity suite that Q-021
  deferred.
- The same run twice in one process gives bit-identical trades and decision
  trace. The decision trace on a prefix of a series equals the full run's
  trace on that prefix.

### Preserved guarantees

- The Q-026 exit-rule gate, the Q-025 frame and window gates, and the Q-022
  indicator gate keep passing unchanged. `q-engine` stays free of unsafe code,
  input and output, clocks and environment access, and its dependency direction
  is unchanged.
- The wheel's existing functions keep their names and behaviour, and it still
  builds without Qt.

## Constraints and non-goals

- **No change to `q_backend`.** The engine swaps to the kernel in Q-028 and the
  forward evaluator to the decision step in Q-031. Until then the Python engine is
  the reference.
- **No indicator or signal computation, and no day chunking.** Indicators and
  decision columns are computed in Python before the kernel runs. The engine's
  day-trade parallel mode recomputes indicators per calendar day, so splitting a
  series into days stays with the caller, which calls the kernel once per day with
  force-close at end set.
- **No metrics, equity curves or trade objects.** Performance metrics, equity
  curves, order and trade ids and timestamps stay in the backend, built from the
  returned columns.
- **No fixes to quirks found along the way.** A queued close for one side closes
  both sides with the first close's reason; the position cap counts both sides;
  sequential runs leave trades open at the end; the evaluator sizes at the bar
  close with its initial capital while the engine sizes at the next open with
  compounded capital. All are reproduced. The step takes price and capital as
  inputs precisely so both sizing policies stay expressible without choosing
  between them.
- **No multi-symbol runs.** A strategy has one symbol today, and every queued
  signal and every trade uses it.
- **No tick kernel.** The tick simulation's sizing, spread fills and stops are a
  different semantic with different rounding, and belong to Q-029. Sharing code
  between them would change one of them.
- **No new sizing models, order types, slippage or partial fills.**
- **No Arrow or Parquet I/O, and no requirement to build a Q-025 frame.** The run
  takes plain columns so that the engine's current input, which may repeat times,
  is never rejected. A frame can supply those columns.
- **No move of the `q_backend` reference pin.**

## Acceptance criteria

### Agent-verifiable

1. Unit tests state the loop order as cases: force-close time preempts fills and
   evaluation; a close queued for longs closes an open short on the same symbol
   with the same reason; exits fill before entries on one bar; the cap trims an
   entry to the remaining quantity and skips it at zero; capital after a closed
   trade changes later safety-margin sizing; a warm-up bar queues nothing.
2. Unit tests state the decision step as cases: rule exits precede strategy exits;
   a fired rule suppresses every strategy exit on the symbol; a holding period of
   2 closes at two bars after entry and not at one; a trade with no entry bar is
   never closed by it; strategy exit columns are ignored while a holding period is
   declared; at most one entry is queued.
3. Unit tests state sizing as cases for all three models: flooring, strength
   scaling to zero opening nothing, minimum and maximum contracts, missing and
   non-positive volatility, and each model's maximum position.
4. Candle-engine reference fixtures exported from `q_backend` at the existing pin
   pass the Q-021 gate under the exact policy for every engine scenario listed
   under Parity. Negative controls each fail: one expected price moved by one ULP;
   one trade's exit bar moved by one; one exit reason changed; a checksum
   mismatch; and an unaccounted fixture file.
5. Queued-signal parity fixtures exported from the backend's reference extractor
   and forward evaluator at the same pin pass the gate when the decision step is
   run one bar at a time, with and without an open trade.
6. The Q-021 double-run determinism check and the prefix check on the decision
   trace pass for every engine scenario.
7. Through the installed wheel, a run over numpy arrays returns trade columns
   equal to the Rust run for one fixture scenario, a decision-step object keyed by
   string trade ids reproduces that scenario's per-bar decisions, a float16 price
   array and a length mismatch each raise an exception naming the argument, and the
   reported required columns for a mapping with ATR and Donchian exits equal the
   backend's list for that mapping.
8. The existing Q-022, Q-025 and Q-026 gates pass unchanged and
   `make parity-isolation` passes.
9. The full validation suite passes: `make check`.

### Human-verifiable

1. The candle-engine and queued-signal fixtures are regenerated from the pinned
   backend in its full environment and show no diff. The regeneration time is
   reported.
   Command: `cd q_core && time make fixtures-backend-check`
2. Kernel throughput is measured against the Python engine at the pin on the same
   machine: one 50,000-bar synthetic series with fixed-quantity sizing and a
   trailing stop, five runs each, reporting individual and median wall times and
   bars per second for both.
   Command: `cd q_core && make bench-candle-kernel`
