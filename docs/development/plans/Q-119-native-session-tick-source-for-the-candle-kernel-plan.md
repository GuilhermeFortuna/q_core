# Q-119 implementation plan: Native session tick source for the candle kernel

> **For implementation agents:** Read the linked spec and repository instructions.
> Use superpowers:executing-plans when that skill is available. Start only through
> `./work start Q-119 --agent <agent> --worktree` after written-plan approval.
> Implement this task natively; delegation requires separate authorization.

**Goal:** The candle kernel reads a bar's trade prices from tick-store session files in Rust, with no Python call per bar.
**Architecture:**
- `q-io` decodes a session's `time_msc` and `last` columns and applies the trade filter.
- `q-engine` adds `SessionTicks`, a pure `IntrabarSource` over a `SessionLoader` trait.
- `q-py` exposes `SessionTickSource` and accepts it as `intrabar`.

**Spec:** [Specification](../specs/Q-119-native-session-tick-source-for-the-candle-kernel-spec.md)
**Status:** written plan awaiting human review.

## Global constraints

- The linked spec defines the interface, rules and acceptance criteria; do not widen scope.
- The callable `intrabar` path is untouched. Existing fixtures, gates and `BACKEND_REV` stay unchanged.
- No I/O in `q-engine`; the loader trait is the boundary.
- Use the repository toolchain and canonical checks without resource-slice wrappers.
- Commit focused changes on the task branch. Never push, merge or change protected branches.

## Ordered implementation

- [ ] 1. `q-io`: add `read_trade_prices` beside the bar readers in `src/parquet.rs`, reusing its path checks and error type. Add unit tests for criterion 1 that write files with `tempfile` and the tick schema. Commit.
- [ ] 2. `q-engine`: add `candle/session_ticks.rs` with `SessionLoader`, `TradePrices` (or a re-export) and `SessionTicks`: one cached session, binary-search slicing, the OHLC check and the three typed errors. Write tests for criteria 3–5 with an in-memory counting loader. Commit.
- [ ] 3. `q-py`: add the `SessionTickSource` class with argument validation, a `q-io` loader and `prices(bar)`. Make `run_candle`'s `intrabar` accept the class or a callable. Add `IntrabarDataError` with its attributes and the `SESSION_TICK_SOURCE` flag.
- [ ] 4. Add `tests/test_engine.py` cases for criteria 2, 5 and 6, including a spy that proves no Python price call. Commit.
- [ ] 5. Document in `README.md`. Run `make check`. Commit.

## Review focus

- The trade filter and the stable sort match `TickStore._ensure_trade_prices_session` exactly, including equal timestamps.
- Interval bounds are half-open in `time_msc`, and the reported times are `time_msc * 1000`.
- Only one session is held in memory, and in-order requests never reload it.
- Error attributes carry enough for the host to rebuild its current messages.
- No new dependency in `q-engine`, and no GIL round trip per bar with the native source.

## Validation and handoff

Run `cargo test -p q-io -p q-engine`, then `make check`. Record the results and follow-ups in this plan and commit. Then run `./work board set Q-119 in-review -m "<changes; checks and results; follow-ups>"` from the workspace root. The human owns integration and the release; Q-121 pins the published tag.
