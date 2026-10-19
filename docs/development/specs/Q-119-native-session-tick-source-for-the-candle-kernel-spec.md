# Q-119: Native session tick source for the candle kernel

**Status:** written spec and plan awaiting human review; status of record is the [Q project board](https://github.com/users/GuilhermeFortuna/projects/2).
**Batch:** 28 — Fast tick-backed backtests
**Depends on:** none
**Implementation plan:** [Plan](../plans/Q-119-native-session-tick-source-for-the-candle-kernel-plan.md)

## Purpose

Serve a bar's trade prices to the candle kernel from the research tick store's session files in Rust, so the kernel never calls back into Python for prices. Today every bar that needs ticks costs a Python call that reads, filters and checks them (Batch 28, finding 3). With resting orders (Q-118) more bars need ticks, and that cost grows with the number of bars.

## Current system

- `run_candle(..., intrabar=callable)` wraps the Python callable in `PyIntrabar` (`crates/q-py/src/engine.rs`). For each bar it needs, the kernel calls the callable with the bar index and gets `(time_us, price)` arrays back.
- In `q_backend` the callable is `TickReplay`. It reads one session's parquet file, keeps trades whose `last` is finite and positive, stable-sorts them by `time_msc` when out of order, and slices out the bar's half-open interval. It then checks that the slice's first, highest, lowest and last prices equal the bar's open, high, low and close. Prices come back with `time_us = time_msc * 1000`.
- `q-io` reads bar parquet files only.

## Required behavior

### Reading a session's trades (`q-io`)

- `read_trade_prices(path) -> TradePrices { time_msc: Vec<i64>, price: Vec<f64> }` reads the columns `time_msc` (int64) and `last` (float64) from one tick-store session file.
- It keeps rows whose `last` is finite and positive, and stable-sorts by `time_msc` only when the column is out of order.
- The result equals `TickStore._ensure_trade_prices_session` for the same file.
- A missing file, a missing column or a wrong dtype returns an `IoError` naming the path and the column.

### Session source (`q-engine`)

- `SessionTicks` implements `IntrabarSource` over:
  - per-bar session ordinals and half-open intervals `[start_msc, end_msc)`, in the store's `time_msc` units;
  - per-bar open, high, low and close;
  - a `SessionLoader` trait that returns one session's `TradePrices`, or "missing".

  `q-engine` stays free of I/O; the loader is the only source of data.
- `prices(bar)` returns the trades whose `time_msc` lies in the bar's interval, with `time_us = time_msc * 1000`, using binary search on the loaded session.
- One session is held at a time. A request for another session replaces it, so memory is bounded by the largest session.
- Errors end the run, as `IntrabarSource` errors do today. There are three typed reasons, each naming the bar:
  - missing session (with the ordinal);
  - empty interval;
  - OHLC mismatch (with the observed first, high, low and last and the bar's open, high, low and close).

### Binding (`q-py`)

- `q_core.engine.SessionTickSource(paths, bar_session, bar_start_msc, bar_end_msc, open, high, low, close)`:
  - `paths` is a list of file paths, with `None` for a session that is not stored;
  - the other arguments are equal-length `int64` or `float64` arrays.

  It composes `SessionTicks` with a `q-io` loader. Construction validates lengths, dtypes and that each ordinal is in range.
- `run_candle(..., intrabar=source)` accepts a `SessionTickSource` as well as a callable. With a native source, the kernel makes no Python call to get prices.
- `source.prices(bar)` returns `(time_us, price)` NumPy arrays, for hosts that replay a bar themselves.
- The three error reasons raise `q_core.engine.IntrabarDataError`, a subclass of `ValueError`, with the attributes `bar`, `reason` (`"missing_session"`, `"empty"` or `"mismatch"`), `session`, `observed` and `expected` (`None` where not applicable). The host can then raise its own domain errors.
- `q_core.engine.SESSION_TICK_SOURCE` is `True`.

### Unchanged

- A callable `intrabar` behaves exactly as today.
- `run_candle` outputs are bitwise identical whichever source serves the same prices.
- Existing fixtures, gates and `BACKEND_REV` do not change.

## Interfaces and ownership

- **`q-io`** owns parquet decoding and the trade filter.
- **`q-engine`** owns interval slicing, the session cache and the OHLC check.
- **`q-py`** owns the Python surface. The host owns paths, sessions and interval bounds.
- **`README.md`** documents the source in the `q-io`, `q-engine` and binding sections.

## Acceptance criteria

The session files are written under a temporary directory with the tick store's schema (`time_msc`, `bid`, `ask`, `last`, `volume`, `flags`).

1. `read_trade_prices` drops rows whose `last` is zero, negative or `NaN`, and stable-sorts an out-of-order file. Equal timestamps keep file order.
2. A Python test proves `read_trade_prices` equals `TickStore._ensure_trade_prices_session` arrays for the same file, using the fixture schema. The test can use a copy of the filter when `q_backend` is not importable.
3. `prices(bar)` returns exactly the interval's trades, at its first and last boundaries, with `time_us = time_msc * 1000`.
4. A counting loader shows one load per session while bars are requested in order, and a reload only when a different session is asked for.
5. A missing session, an empty interval and an OHLC mismatch each raise `IntrabarDataError` with the specified attributes.
6. `run_candle` with a `SessionTickSource`, and with a callable serving the same prices, returns identical results for the Q-102 and Q-105 fixtures. A spy shows that no Python call is made for prices with the native source.
7. Existing fixtures and gates pass unchanged.
8. `make check` passes.

## Implementation boundary

This issue authorizes only its listed deliverable after written-plan approval and
`./work start Q-119 --agent <agent> --worktree`.

Out of scope:
- aggregating bars from ticks;
- quote or bid/ask sources;
- the substitution rules of Q-107's stack replay;
- any change to the tick kernel, the live evaluator, contracts or the Qt binding.
