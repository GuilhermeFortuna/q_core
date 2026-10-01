# Q-081: Deterministic volume analysis kernels

**Status:** written spec and plan awaiting human review; status of record is the [Q project board](https://github.com/users/GuilhermeFortuna/projects/2).
**Batch:** 13 — persistent terminal setup and live market analysis
**Depends on:** Q-074, Q-079
**Implementation plan:** [Plan](../plans/Q-081-deterministic-volume-analysis-kernels-plan.md)

## Purpose

Provide shared deterministic calculations for tape-derived volume studies.

## Current system

q-indicators contains pure batch and streaming price studies; q-engine tick_bars aggregates OHLCV for simulation but has no aggressor-volume or tape-activity state. Reuse streaming conventions without changing simulation aggregation or existing fixture outputs.

## Required behavior

- Add q_indicators::volume with typed TradeInput, AggressorSide, VolumeConfig, VolumeState and VolumeOutput. Consume validated trades in source order plus explicit session keys and chart bar intervals; do not interpret clocks, timezones, wire envelopes, volume units or record identities.
- TradeInput contains time_msc, price, volume, raw_flags, session_key, bar_open_msc and bar_close_msc. Side is Buy when BUY is set without SELL, Sell for the inverse, otherwise Unknown. Equal timestamps are valid and processed separately in supplied order.
- Bar volume accumulates buy, sell and unknown quantities. Delta = buy minus sell. Cumulative delta sums delta from the supplied session start and resets when session_key changes. Report classified share = (buy + sell) / total; use unavailable for a zero denominator. All-unknown trades produce classified delta 0 with classified share 0, never a bullish/bearish interpretation.
- Trade rate is eligible trade count in (now_msc - window_ms, now_msc] divided by window_ms/1000. Default window_ms=10000, valid 1000..300000. now_msc is explicitly supplied and may advance without a trade. Return warm-up until a full continuously observed window exists.
- A large print is one individual trade with volume >= large_print_threshold; default 100, any finite positive threshold is valid. Do not reconstruct child trades into orders. Emit side, time, price and volume for each qualifying print; replay produces the same output.
- VolumeState::new(config), push(trade), advance(now_msc), current(), reset() and batch_volume(config, trades, checkpoints) share one calculation path. Reconfiguration is reset/replay, not mixed accumulated semantics.
- Delta/CVD state retains constant-sized accumulators. The time-window deque has max 100000 events; exceeding it returns an explicit capacity error before mutating state, never drops prints silently. Terminal Q-082 marks the reading unavailable until a valid replay with adequate capacity/rate settings.
- Reject non-finite/nonpositive price/volume, invalid intervals, decreasing time, invalid config and time checkpoints before the last input; leave state unchanged on rejected inputs. A session change flushes the activity window. Invalid coverage is host-owned and cannot be fixed by the kernel.
- Compute bitwise-equal batch/incremental outputs using the same source order. Test prefix causality and double-run determinism. No I/O, system clock, environment, scripting, trade recommendation or Python binding is added.

## Interfaces and ownership

q-indicators owns volume arithmetic and side semantics; q_terminal owns source coverage and deduplication.
Expose TradeInput, AggressorSide, VolumeConfig, VolumeState, VolumeOutput, LargePrint and VolumeError from crates/q-indicators/src/lib.rs. VolumeOutput includes bar bounds, buy/sell/unknown volumes, delta, cumulative_delta, classified_share and rate/warm-up status. Q-082 consumes the released q_core tag directly.

## Acceptance criteria

1. A fixture of buy 120, sell 30 and unknown 50 produces delta/CVD 90, total 200 and classified share 0.75; a new session resets CVD.
2. Both/neither aggressor flags remain unknown; equal identical same-millisecond inputs both contribute.
3. Trade-rate boundary fixtures exclude the left endpoint, include the right, age out on explicit advance and show warm-up for incomplete windows. Threshold equality marks a large print.
4. Batch and incremental output are bitwise equal for every fixture prefix; repeated replay is deterministic and appending future trades does not change earlier output.
5. Invalid input/config, decreasing time and capacity overflow leave prior state intact.
6. make check passes; human work finish publishes the tag for Q-082.

## Implementation boundary

This issue authorizes only its listed deliverable after written-plan approval and
`./work start Q-081 --agent <agent> --worktree`. Dependencies must be Done.
Preserve the existing execution controls and research/operations ownership boundaries.
No live-order activation, new backend-process ownership or unrelated refactoring.
Use generated contracts and commit/tag pins; never edit vendored code by hand.
