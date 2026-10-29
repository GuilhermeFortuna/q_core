# Q-138: Live-placeable orders in the candle kernel

**Status:** written spec and plan awaiting human review; status of record is the [Q project board](https://github.com/users/GuilhermeFortuna/projects/2).
**Batch:** 39 — Live-placeable research orders
**Depends on:** none
**Implementation plan:** [Plan](../plans/Q-138-live-placeable-orders-in-the-candle-kernel-plan.md)

## Purpose

Research strategies get precise intrabar fills from same-candle priced orders (`price=`). The hook decides after
seeing the whole candle, and the order fills inside it. A broker cannot hold such an order. Batch 39 moves
research to orders placed before the candle and held by the simulated broker, as MT5 holds them live. This task
adds the kernel primitives that keep that model as precise as today's:
- one-cancels-the-other groups of resting entries;
- timed exits and timed expiry;
- a placement latency.

Every existing order type keeps exactly its current fills.

## Current system

- `crates/q-engine/src/candle/`:
  - resting stop and limit entries with `expire_bars` (one working entry at a time);
  - protective `stop_loss`/`take_profit` with amendments (`protective.rs`);
  - priced entries and order lists that fill from the deciding candle's ticks;
  - the Q-132 rule that a priced entry after a same-bar exit fills only from a trade at or after that exit.
- Exit rules (`crates/q-engine/src/exits/`) include trailing and breakeven stops, updated from ticks.
- Fills follow the documented rules. A stop fills at the first trade at or beyond its level, at that trade's
  price. A limit or target fills at its level once a trade goes strictly beyond it.
- `crates/q-engine/tests/*_gate.rs` and `fixtures/reference/` hold the golden behaviour. `q-py` exposes the kernel
  to `q_backend`.

## Behaviour

### One-cancels-the-other entries

- A decision may place an **OCO group**: two or more resting stop or limit entries, each with its own side,
  level, protective levels, reason and expiry. The first one the ticks fill opens the position, and the others
  are cancelled at that tick. The group replaces any working entry, as a single resting entry does today.
- On a bar where a trade closed intrabar, a group member fills only from a trade at or after that exit (the
  Q-132 rule).
- A group whose members would fill on the same tick resolves to the member listed first, as order lists do today.

### Timed orders

- A **timed exit** closes the open position at the first trade at or after a given instant, at that trade's
  price. It is placed by a decision like any other order, and replaced or cancelled by later decisions.
- Resting entries accept an **expiry instant** besides `expire_bars`. A resting entry is cancelled at its expiry
  instant if it has not filled.
- A timed exit whose instant falls inside a bar with ticks fills from those ticks. If the bar's first trade is at
  or after the instant, the timed exit fills at the bar's open: that is today's "close at this bar's open" result.

### Placement latency

- A run takes a **placement latency** in microseconds, defaulting to 0. An order decided at a bar's close fills
  only from trades at or after the close plus the latency. With the default, every fill is unchanged.

### Unchanged behaviour

- Market orders, priced entries and closes, order lists, resting entries, protective levels, amendments and exit
  rules keep their exact current fills. No existing golden fixture or gate expectation changes.

### Python surface (`q-py`)

- The kernel's Python entry points accept OCO groups, timed exits, expiry instants and the latency. The new
  arguments are optional, so a caller that passes none of them gets byte-identical results.

## Acceptance criteria

1. Every existing test and golden fixture in `crates/q-engine` and `q-parity` passes unchanged.
2. **Equivalence gates** (new, in `crates/q-engine/tests/`). Each runs a legacy form and a new form on the same
   fixture bars and ticks, and requires identical ledgers (entry and exit times, prices, reasons and quantities):
   - a priced entry list at levels fixed by the previous bar, against an OCO group of resting stops placed at the
     previous close with a one-bar expiry. This includes a bar where an earlier trade exited intrabar;
   - a priced close at the current bar's open once its open passes a time, against a timed exit at that time;
   - a priced stop close at a level fixed by the previous bar, against a protective stop set by amendment at the
     previous close.
3. Gates for the new behaviour:
   - an OCO group where the second member fills first;
   - members filling on the same tick;
   - a group replaced while working;
   - expiry by instant;
   - a timed exit on a bar without a trade after the instant (it fills on the next bar's first trade);
   - a non-zero latency that skips trades before the close plus the latency.
4. Runs passing no new arguments return byte-identical results through `q-py`.
5. `make check` passes.

## Out of scope

- Removing priced orders (Q-141), and the research library API (Q-139).
- Decision timeframes finer than the bar timeframe (Q-140).
- Live order submission (Q-137).
