# Q-111: Research indicator kernels

**Status:** written spec and plan awaiting human review; status of record is the [Q project board](https://github.com/users/GuilhermeFortuna/projects/2).
**Batch:** 23 — Research indicators
**Depends on:** none
**Implementation plan:** [Plan](../plans/Q-111-research-indicator-kernels-plan.md)

## Purpose

Add the indicator kernels that research needs and `q-indicators` lacks: log returns, per-bar volatility estimators, volatility-normalized scores, a log-price z-score, a volatility-regime ratio and quoted bid-ask spread aggregated to bars.

## Current system

`q-indicators` holds 16 batch kernels with pandas' semantics, among them `atr`, `realized_vol` (centred, annualized), `yang_zhang` and `rolling_zscore`. `yang_zhang` with `periods_per_year = 1` already is the per-bar Yang–Zhang estimator. Nothing computes log returns, an exponentially weighted volatility, Rogers–Satchell or any quote statistic; `tick_bars` builds OHLC from last or mid prices only.

## Required behavior

### Conventions

- Kernels take `&[f64]` columns and return `Vec<f64>` of the input length. NaN is the only missing value.
- Logarithms are natural. A log-based kernel needs finite, strictly positive prices; an invalid bar gives NaN, a rolling window that contains it is NaN, and an exponentially weighted kernel restarts from the next valid bar.
- Volatility is per bar, in log-return units, never annualized.
- A volatility at or below `1e-12` is a valid output and an invalid denominator: the quotient is NaN.
- Half-life `H` (finite, positive) gives `lambda = exp(-ln 2 / H)`. The recursion is seeded with the first valid input and its output is NaN until `N_EW(H) = 1 + ceil(H * log2(100))` inputs have been seen, so the seed weighs at most 1%.
- No value is rounded, clipped or filled.

### Kernels

With `l_t = ln(C_t)` and `r_t = l_t - l_(t-1)`:

| Kernel | Definition | First value at index |
| --- | --- | --- |
| `log_return(close, horizon)` | `l_t - l_(t-h)`, `h >= 1` | `h` |
| `realized_vol_per_bar(close, window)` | `sqrt(sum(r^2 over n) / n)`, mean not removed, `n >= 1` | `n` |
| `ewma_vol_per_bar(close, half_life)` | `v = lambda * v + (1 - lambda) * r^2`, seeded with the first `r^2`; output `sqrt(v)` | `N_EW(H)` |
| `rogers_satchell(open, high, low, close, window)` | `sqrt(mean over n of u(u-c) + d(d-c))` with `u = ln(H/O)`, `d = ln(L/O)`, `c = ln(C/O)`, `n >= 1` | `n - 1` |
| `true_range(high, low, close)` | `max(H - L, abs(H - C_prev), abs(L - C_prev))` | `1` |
| `atr_fraction(high, low, close, period)` | the existing `atr` divided by `close`; NaN unless `close > 0` | as `atr` |
| `price_zscore_log(close, window)` | `(l_t - mean) / std` over the trailing `n` log closes, sample `std` (`ddof = 1`), `n >= 2` | `n - 1` |
| `ewma_trend_spread(close, fast_half_life, slow_half_life)` | `EW(l, fast) - EW(l, slow)`, both seeded with the first log close, `fast < slow` | `N_EW(slow) - 1` |
| `vol_normalize(values, sigma, horizon, vol_lag)` | `values_t / (sigma_(t - vol_lag) * sqrt(horizon))`, `horizon >= 1`, `vol_lag` in `{0, 1}` | when both inputs are valid |
| `volatility_log_ratio(sigma_short, sigma_long)` | `ln(sigma_short) - ln(sigma_long)` | when both inputs are valid |

`vol_normalize` builds the three normalized scores from one kernel: momentum (`log_return(h)` with `horizon = h`), trend (`ewma_trend_spread` with `horizon = 1`) and spread against volatility. `price_zscore_log` and `volatility_log_ratio` apply the `1e-12` rule to their own denominators.

`yang_zhang` and `atr` are unchanged. `README.md` states that `yang_zhang(..., periods_per_year = 1)` is the per-bar estimator.

### Quoted spread

`quote_spread_bars(time_msc, bid, ask, flags, session, bar_open_msc, bar_close_msc, max_quote_age_ms)` returns four columns of the bar count: `last`, `time_weighted_mean`, `max` and `coverage`. Spread values are relative: `(ask - bid) / ((ask + bid) / 2)`.

- Ticks arrive in source order with non-decreasing times; equal times keep their order. Bars are `[open, close)`, ordered and non-overlapping; a tick at a bar's close belongs to the next bar.
- A tick is a quote update when `flags` has the MQL5 bid bit (`2`) or ask bit (`4`). Other ticks neither change the quote nor refresh its age.
- A quote is valid when `0 < bid <= ask` and both are finite. An invalid quote update ends the carried quote until the next valid one. The quote is never repaired, swapped or rebuilt from `last`.
- A valid quote is carried until the next quote update, a change of `session`, or `max_quote_age_ms` after its update, whichever comes first. Expiry needs no tick.
- `time_weighted_mean` weights each carried value by its duration inside the bar; `max` is the largest value with positive duration; `coverage` is the carried duration divided by the bar's duration. A bar with no carried duration has NaN mean and max and coverage `0`.
- `last` is the quote carried at the bar's close, or NaN when none is.
- The caller supplies the session key and decides the minimum coverage it accepts; the kernel reads no clock or calendar.

### Determinism and rejection

- Every kernel is bit-identical on a double run and prefix-causal: the result on a prefix equals the prefix of the result. No parallelism, fused multiply-add or reordered sums.
- Out-of-domain parameters and unequal column lengths return an `IndicatorError` naming the function, parameter and value. Nothing panics.

## Interfaces and ownership

- `q-indicators` owns every formula. The existing 16 kernels, their fixtures and `BACKEND_REV` do not change.
- `q-py` exposes each kernel in `q_core.indicators` over contiguous float64 numpy arrays (`flags` as int64, times as int64), with no semantics in the binding. `quote_spread_bars` returns a tuple of four arrays.
- `README.md` documents the conventions and the quote rules.

## Acceptance criteria

Hand-computed fixtures on small synthetic series.

1. `log_return` on `[100, 110, 121]` ends with `ln 1.1` for `h = 1` and `ln 1.21` for `h = 2`; a flat series gives exactly `0`.
2. Returns `[0.01, -0.01, 0.01, -0.01]` with `n = 4` give `realized_vol_per_bar` `0.01`.
3. `ewma_vol_per_bar` with `H = 32` is NaN before index 214 and equals a plain recurrence afterwards; with `H = 1` the state after returns `0.01, 0.02` is `v = 0.00025`.
4. On bars `(100, 102, 98, 101)`, `(103, 106, 102, 105)`, `(104, 105, 101, 102)` after a close of 99, `rogers_satchell` with `n = 3` gives `0.02533109341314` and `yang_zhang` with `n = 3`, `periods_per_year = 1` gives `0.02896649184455`, both within `1e-12`.
5. Previous close 100, high 106, low 103 give `true_range` 6; the first bar is NaN. `atr_fraction` equals `atr / close` element by element.
6. `price_zscore_log` on `[1, e, e^2]` with `n = 3` ends with `1`; a flat window and a window whose standard deviation is at most `1e-12` give NaN.
7. `ewma_trend_spread` is `0` on a flat series and rejects `fast >= slow`.
8. `vol_normalize` of a two-bar return `2a` with `sigma = a` and `horizon = 2` gives `sqrt(2)`; `sigma <= 1e-12` gives NaN; `vol_lag = 1` reads the previous bar's `sigma`.
9. `volatility_log_ratio` of `0.02` and `0.01` gives `ln 2`.
10. Bid 99.9 and ask 100.1 give a relative spread of `0.002`. Ten seconds at 10 bps then fifty at 20 bps in a 60-second bar give mean `18.3333…` bps, max 20 bps and coverage `1`.
11. Quote rules: a quote expires without a new tick and lowers coverage; a trade-only tick does not refresh it; a crossed quote ends it; a session change ends it; a tick at the bar's close counts in the next bar.
12. Multiplying all prices by a positive constant leaves every output but `true_range` unchanged. Every kernel passes the double-run and prefix checks. A non-positive close gives NaN and restarts an exponentially weighted kernel.
13. The Python binding returns the Rust values for criteria 4 and 10 and raises `ValueError` for an invalid parameter.
14. The existing reference gate still reports 16 bound and 0 pending, and `make check` passes.

## Implementation boundary

This issue authorizes only its listed deliverable after written-plan approval and
`./work start Q-111 --agent <agent> --worktree`.
No streaming state, no Qt binding, no contract change, no change to an existing kernel or fixture, no roll or gap detection, no reason codes for missing values, no annualized variants and no effective or order-book spread.
