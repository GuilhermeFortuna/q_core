# Q-083: Explainable market context kernels

**Status:** written spec and plan awaiting human review; status of record is the [Q project board](https://github.com/users/GuilhermeFortuna/projects/2).
**Batch:** 13 — persistent terminal setup and live market analysis
**Depends on:** Q-074
**Implementation plan:** [Plan](../plans/Q-083-explainable-market-context-kernels-plan.md)

## Purpose

Produce deterministic, inspectable market-context readings from the existing price studies.

## Current system

Streaming EmaState, RsiState, AtrState, SmaState and SessionVwapState already exist in q-indicators. No shared market-context evaluator currently classifies their readings; QML must not independently invent classifications.

## Required behavior

- Add q_indicators::context with ContextConfig, ContextInput, ContextState and ContextOutput. Reuse Q-074 study states; expose batch_context plus commit, non-mutating preview and reset. A caller supplies session keys and VWAP availability, never a kernel clock/calendar.
- Independent defaults: fast EMA 9, slow EMA 21, RSI 14, RSI lower/upper 30/70, ATR 14, ATR baseline SMA 20, volatility lower/upper ratios 0.8/1.2 and VWAP extension 1 ATR. Price source is close; VWAP uses typical price and consistent actual volume.
- Trend: upward when fast > slow and both one-bar slopes > 0; downward when fast < slow and both slopes < 0; balanced when fast equals slow and both slopes are zero; mixed otherwise. Return the EMA values, slopes and fast/slow spread divided by ATR as evidence; absent ATR only removes normalized evidence.
- Momentum: RSI < lower is lower_zone, RSI > upper is upper_zone, otherwise middle_zone (threshold equality is middle). Its change is the current RSI minus the preceding committed-bar RSI, with rising/falling/unchanged labels. Missing preceding RSI makes change unavailable.
- Volatility: ratio = ATR / SMA(ATR, baseline_period); < lower is contracting, > upper expanding, equality/between typical. Reject nonpositive/nonfinite baseline as unavailable instead of inventing a ratio.
- VWAP: signed distance = (close - VWAP)/ATR; below/at/above is independent of the distance availability. Extended when abs(distance) >= extension_threshold; otherwise near. ATR zero makes normalized distance unavailable; unavailable VWAP does not hide trend/momentum.
- Warm-up and unavailable status are per reading. Preview compares current provisional values against the preceding committed output, never against another preview. Repeated forming updates do not mutate state or emit events.
- Config validates periods 1..1000, fast < slow, 0 < rsi_lower < rsi_upper < 100, 0 < volatility_lower < volatility_upper and positive VWAP extension, with finite floats. Config changes require full reset/replay.
- ContextOutput carries typed categories plus numeric evidence/thresholds and per-reading reasons. It provides no blended score, forecast, recommendation, order signal, model access or scripted formulas.
- Batch/incremental parity is bitwise under the same inputs. Existing indicator outputs, reference fixtures and bindings remain unchanged. Publish through the normal q_core release for Q-084.

## Interfaces and ownership

q-indicators owns context math/categories; q_terminal owns target, freshness, coverage, history and text rendering.
ContextInput supplies OHLC, optional volume, session_key and vwap_available. OHLC must be finite, positive and consistent (low <= open/close <= high); present volume must be finite and nonnegative. Missing volume makes only VWAP unavailable. Validate the entire input before advancing any state. ContextOutput exposes TrendReading, MomentumReading, VolatilityReading and VwapReading with typed status/category/evidence. Add crates/q-indicators/src/context.rs and exports; no wire schemas or Python/Qt-specific logic.

## Acceptance criteria

1. Hand-computed fixtures cover every category, mixed trends, flat/equal inputs and threshold equality; numeric evidence agrees with the existing study kernels.
2. Every prefix has bitwise batch/commit parity; preview equals committing the same input and repeated previews leave state unchanged.
3. RSI/ATR warm-up, zero baselines, unavailable VWAP and session change affect only the appropriate readings with stated reasons.
4. Invalid config/input leaves prior state intact; double-run determinism and prefix causality hold.
5. Existing fixture/reference gates and make check pass; the human publishes a q_core tag for Q-084.

## Implementation boundary

This issue authorizes only its listed deliverable after written-plan approval and
`./work start Q-083 --agent <agent> --worktree`. Dependencies must be Done.
Preserve the existing execution controls and research/operations ownership boundaries.
No live-order activation, new backend-process ownership or unrelated refactoring.
Use generated contracts and commit/tag pins; never edit vendored code by hand.
