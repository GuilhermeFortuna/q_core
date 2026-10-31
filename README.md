# `q_core`

Deterministic computational core for the Q trading platform.

`q_core` provides a single authoritative implementation of every market, simulation, and execution semantic that research backtesting and live trading depend on. It is consumed by two hosts:
- Python (`q_backend` and execution workers) via a native wheel built with PyO3/maturin.
- Qt/QML (`q_terminal`) via a native bridge built with CXX-Qt.

Neither host embeds computational semantics; all deterministic logic lives in Rust within this workspace.

---

## Tape Volume Analysis

`q_indicators::volume` provides incremental and batch trade-volume analysis through
`VolumeState` and `batch_volume`. Callers supply trades in source order, including
their session key and chart-bar open/close timestamps. The kernel uses the supplied
price and volume units unchanged; it does not convert units, infer missing source
coverage, or deduplicate trades. Equal-millisecond trades are processed individually.

Buy and sell classification uses the MQL5 `TICK_FLAG_BUY` and `TICK_FLAG_SELL`
bits (`1 << 5` and `1 << 6`). Exactly one side bit classifies the trade; both or
neither means unknown. Delta excludes unknown volume, while classified share reports
classified volume divided by total volume (`None` when total volume is zero).
Cumulative delta resets when the caller changes the session key.

Trade rate is trades per second in the open-left, closed-right interval
`(now_msc - window_ms, now_msc]`. It is unavailable until the caller has supplied
a full window of continuous observation. Call `advance(now_msc)` to age the window
when no trades arrive. The rate deque holds at most 100,000 events; exceeding that
capacity returns `VolumeError::CapacityExceeded` without changing state. To change
configuration, create a new state and replay the desired input history.

## Runtime candle strategy decisions

`q_engine::run_candle_with_callback` uses the same simulation loop as `run_candle`.
Its fallible callback receives a bar index and `PositionSnapshot` values (ledger key,
side, entry bar, entry price, quantity), after queued fills and intrabar protective
fills and before current-bar decisions. It returns `BarSignals`; the existing engine
still applies exit-rule precedence, session gates, sizing, and execution costs.
Callbacks run once per bar, including suppressed and final bars. Orders remain queued
until the next open, and end-of-day or terminal closes may happen after the callback.

Python callers may pass `strategy_callback` to `q_core.engine.run_candle`. It receives
`(bar, positions)`, where positions is a tuple of
`(ledger_ordinal, side, entry_bar, entry_price, quantity)` tuples (`side` is `1` or `-1`).
It returns `(entry, exit_long, exit_short, strength)`, or
`(entry, exit_long, exit_short, strength, stop_price, target_price)` to give an entry decided
on this bar its own levels, optionally followed by its `entry_price` (a seventh value; see
[Priced entries](#priced-entries-q-enginecandle)). `NaN` means no level or price; without the
extra values the static columns apply. Entry is `-1`, `0`, or `1`, exit flags are booleans,
strength must be finite and within `[0, 1]`, and levels and prices must be `NaN` or finite and
positive. Runtime levels need the `intrabar` source, as static ones do. Invalid decisions or
callback exceptions abort the run. Callback runs own copies of input arrays so host mutations
cannot change the simulation. An eight-value tuple may append `exit_price` after `entry_price`;
the `dict` form can set `exit_price` directly. Calls without runtime arguments retain the static
signal path. This native extension requires no contract or Qt API changes.

The callback `dict` may include `exit_price` alongside `exit_long` or `exit_short` to request an
explicit close. It requires `intrabar`, `open`, `high` and `low`; the finite positive price must
lie inside that bar's range. The kernel first applies protective fills, then scans the bar's
trades in stored order by tick index, independent of timestamps. A resting or priced position opened
on this bar starts its close scan after its `first_tick`, so neither earlier equal-timestamp trades
nor its fill print can close it. Other open positions scan from the bar's first trade. The close
fills by the [priced fill rule](#priced-fill-rule): closing a long sells and closing a short buys.
If no trade fills it, the position stays open and the decision does not fall back to a next-open
close. Without `exit_price`, signal closes still fill at the next bar's open. Rust
callbacks set `BarSignals::exit_prices`.

### Priced fill rule

A priced close or entry is a stop or a limit, told apart by the side of the first scanned trade:

- **Stop:** the price moves toward the order, as for a buy above that trade or a sell below it,
  including a long's stop below the market. It fills at the first trade at or beyond its price, at
  that trade's price, so a jump through the level fills at the jump.
- **Limit:** the price comes back to the order, as for a buy below that trade or a sell above it,
  including a long's target above the market. It fills at its own price once a trade goes strictly
  beyond it, since a trade exactly at the price may not reach the order in the queue. Protective
  targets follow the same rule.
- A first scanned trade at the price fills there. A strategy whose bar opens beyond its level
  passes the open as the price.

### Order lists

A decision may offer several prices and let the bar's tape choose:

- **Exits.** `exit_prices` (Rust `BarSignals::exit_prices`) lists the close's candidate prices; the
  `dict` form's `exit_price` is a list of one, and the two keys cannot be combined. Each position
  closes at the candidate its scan reaches first, by the rule above; a tie on one trade goes to the
  earlier listed price. `close_next_open` keeps the next-open close when no candidate is reached.
  The ledger's `exit_candidate` is the winning index, `-1` for every other exit.
- **Entries.** `entry_candidates` (Rust `BarSignals::entry_candidates`) lists priced market entries
  as `(side, price, stop_price, target_price)`, `NaN` for an unset level. They require a market
  entry decision (`entry` of `1` or `-1`; a resting order cannot be combined with them) and cannot
  be combined with a market `entry_price`, which is a list of one. Every price must lie in the
  bar's range. With more than one candidate, or after a same-bar exit, the kernel reads the tape:
  the candidate touched first fills, the earlier listed one on a tie, and the others are cancelled.
  The ledger's `entry_candidate` is the winning index, `-1` for every other entry. If none is
  touched after a same-bar exit, each is reported in `rejected` with reason `1`.
- **Split decisions.** `entry_callback` (Rust `RuntimeCallbacks::entry`) decides entries after the
  strategy callback's priced closes resolve, so it receives the positions left open then; the
  strategy callback then decides exits only. Each callback must leave the other's fields unset.
  Without `entry_callback` a single callback decides both, as before.

## Intrabar Protective Orders (`q-engine::candle`)

`q_core.engine.SessionTickSource` can serve candle trade prices directly from tick-store session
Parquet files. It reads `time_msc` and `last`, filters invalid trade prices, stably orders ticks by
timestamp when needed, and keeps one session loaded at a time. Per-bar intervals are half-open in
milliseconds; the source checks the selected prices against each bar's OHLC before returning
microsecond timestamps to the kernel. Pass the source as `intrabar` to avoid a Python call for each
bar. Callable sources continue to work as before.

`run_candle` can fill an entry's stop and target inside the bar where they trade. Both levels are
optional per-bar absolute prices in `protective`: an entry queued on bar `b` carries the levels of
bar `b`, and `NaN` means no level. Requires `intrabar`, `open`, `high` and `low`; levels must be
`NaN` or finite and positive.

Between sections C and D of each tradable bar that is not force-closed:

1. **Entry check (section C).** A queued entry whose level is already on the wrong side of its fill
   is not opened and is reported in `rejected`. A long needs `stop < fill` and `target > fill`; a
   short needs `stop > fill` and `target < fill`. Only the levels that are set are checked.
2. **Screen.** A long's stop is reached when `low <= stop` and its target when `high > target`; a
   short's stop when `high >= stop` and its target when `low < target`. The source is called only
   when some open trade's level passes the screen, and at most once per bar.
3. **Walk.** The bar's trade prices are walked once, in order. At each price, screened trades are
   checked for their stop (first, then target) and a stop fills at the traded price, so a bar that
   opens past the level fills at its open. A target fills at its level once a price trades strictly
   through it; a price equal to the target does not fill. A trade opened on this bar skips its first
   price, which is its own fill. Protective exits resolve before a resting fill at the same price.
   The walk continues while positions or a reachable resting entry remain.
4. **Close.** The trade closes at the triggering fill price, charged its exit-side cost, and its
   time is recorded in `exit_time_us`. A scan with no triggering price leaves the trade open.
   A source that returns no prices, or fails, ends the run with `CandleError::IntrabarSource` naming
   the bar.

Section D sees a protected trade as closed. Without `protective`, every output is unchanged.

## Priced entries (`q-engine::candle`)

`run_candle` can fill an entry on the bar that decided it, at its own price. The optional
`entry_price` column (`NaN` = no price) is read only on a bar whose decision queues an entry.
A priced entry is not queued: section D of its deciding bar opens it at exactly that price,
sized on the capital at that point and charged its entry cost at that price. The trade is
recorded with the deciding bar as its entry bar. The decision is still traced on that bar.

- Validation runs before any bar is simulated. `entry_price` requires `open`, `high` and `low`,
  has the series length, and holds `NaN` or finite positive values. Levels set beside an entry
  price require `intrabar`; an entry price alone does not.
- The price must lie within `low` and `high` of the deciding bar. Otherwise the run ends with
  `CandleError::EntryPriceOutsideRange`, which names the bar, side, price, low and high.
- The entry check of the protective section applies against the price. A refused entry is
  reported in `rejected` with the price as its fill price. `rejected_reason` is `0` for a
  stop or target on the wrong side of the fill, `1` when an entry has no fill after an earlier
  same-bar exit, and `2` when the bar's trades never fill it.
- With an `intrabar` source the entry fills on the bar's trade prices by the
  [priced fill rule](#priced-fill-rule), scanning from the bar's first trade; its trade is the
  touch, and the entry's price is the fill's. Prices up to and including the touch are ignored when
  its own stop and target resolve after it. Without a source the entry fills at its price.
- When one or more trades close during the bar's protective scan, or a priced signal close fills
  later in section D, a priced entry scans from the latest such exit's trade index, inclusive, so
  prices before the exit do not count. If nothing fills it after the exit, the entry is refused and
  reported with reason `1`; the run continues.
- A close queued by the same decision, such as the exit of a reversal, closes only the trades the
  decision saw open. The priced entry stays open. Position caps count the open trades when the
  entry is sized, so a priced entry beside a position that the queued close will exit is refused
  when the cap has no room for both.
- The closed-bar callback runs before the priced entry fills, so it never sees that trade.
  `RuntimeCallbacks::entry_filled` (Python: `entry_fill_callback`) is then called once with the
  positions open after the fill, the new trade among them, unless that trade already closed on
  its own bar. Returning `true` queues a close of every open trade for the next bar's open, which
  fills ahead of that bar's protective scan. Python callers must return a `bool`.
- Custom intrabar exits do not run on the entry's own bar. They apply from the next bar.
- Without `entry_price` every output is unchanged. Day-trade windows gate priced entries as they
  gate every entry, and priced entries never rest beyond the deciding bar. Resting orders are
  described below.

## Resting orders and amendments (`q-engine::candle`)

A runtime decision may place one resting entry order in place of a market or priced entry. Its
`entry_kind` is `stop` or `limit`, and its `entry_price` is the order price, which must be finite
and positive. `expire_bars` is a positive integer, or none for an order that lasts until it fills,
is replaced or is cancelled. Resting orders require `intrabar`, `open`, `high` and `low`.

- **Placement.** The order is placed in section D of the deciding bar, after the day-trade entry
  window. A gated decision places nothing. The order works from the first price of the next bar.
- **One working order.** Any new entry decision replaces the working order, and `cancel_entry`
  removes it. A stop fills at the first trade price at or beyond its price, at that price. A limit
  fills at its own price once a trade prints strictly beyond it, so an equal print does not fill.
- **Expiry.** An order placed on bar `i` with `expire_bars = k` can fill on bars `i+1` to `i+k`.
  In day-trade mode a working order also lapses at the force-close bar and after the last bar of
  the day, so it never carries into the next session.
- **Reach.** A bar's trade prices are read only when its range reaches the order: a buy stop needs
  `high >= price`, a buy limit `low < price`, a sell stop `low <= price` and a sell limit
  `high > price`. A bar the range cannot reach makes no source call.
- **Fill.** The trade is sized with the capital at that moment and trimmed to the position cap.
  An order the cap leaves nothing for is consumed without a trade and is not reported as rejected.
  Otherwise the wrong-side rule is checked against the actual fill price, and a refused order is
  reported in `rejected` with that price. An entry the sizing rule refuses is still reported when
  its levels are on the wrong side, as before.
- **Amendments.** A decision's `amend_stop` and `amend_target` replace the levels of the open trades
  its callback saw, from the first price of the next bar. `NaN` (`None` in the `dict` form) keeps a
  level. An amendment never reaches an entry decided in the same call, never removes a level and
  never changes exit rules or breakeven state. A level already beyond the next open fills at that
  open.
  Amendments require `intrabar`, `open`, `high` and `low`; the kernel rejects a decision without
  those inputs rather than accepting levels it cannot enforce.
- **Same price.** Within one bar's scan, protective exits resolve before the resting fill. A
  protective exit and a reversing resting entry on one price therefore
  close, then open. A new trade's levels start on the following price, and it never exits on the
  price that filled it.
- **Callback form.** The callback may return a `dict` with the keys `entry`, `exit_long`,
  `exit_short`, `strength`, `stop_price`, `target_price`, `entry_price`, `entry_kind` (`market`,
  `stop` or `limit`), `expire_bars`, `cancel_entry`, `amend_stop`, `amend_target` and `exit_price`.
  Missing keys take their defaults. An invalid value or an unknown key raises `ValueError` naming
  the bar and key. `engine.RESTING_ORDERS` is `True`.
- **Ledger.** Trades gain `entry_kind` (`0` open fill, `1` priced, `2` stop order, `3` limit order),
  `entry_time_us` (the time of the price that filled a resting order, otherwise `-1`), and
  `final_stop_price` and `final_target_price`, the levels in force when the trade closed. With
  `snapshot_levels=True`, each position tuple also carries the trade's current stop and target.

## Incremental execution primitives (`q-engine::execution`)

`q_engine::execution` and `q_core.execution` match one order against one observed market event, and
compute the currency fee of one fill. They are stateless: the caller owns orders, accounts, queues,
clocks, expiry, OCO and activation. The capability is advertised as
`q_core.execution.INCREMENTAL_EXECUTION == 1`.

Units: prices are positive integer ticks, quantities are non-negative integer volume steps, and
decimal metadata (`volume_step`, `tick_size`, `point_value`, rates, fees) is an exact decimal string.
Floats are never accepted for decimal arguments.

- `match_quote(*, side, kind, level_ticks, bid_ticks, ask_ticks, slippage_ticks=0) -> int | None`.
  `side` is `"buy"` or `"sell"`; `kind` is `"market"`, `"stop"` or `"limit"`. A market order takes
  `level_ticks=None`; a stop or limit needs a positive level. A quote is valid when `0 < bid <= ask`.
  Buys trade at the ask and sells at the bid. A buy stop triggers when `ask >= level`, a sell stop
  when `bid <= level`, and a triggered stop fills as a market order. A buy limit is executable when
  `ask <= level` and a sell limit when `bid >= level`; each fills at the executable quote, so it can
  improve on its level. Adverse `slippage_ticks` (non-negative) applies to market and stop fills only.
- `match_trade(*, side, kind, level_ticks, trade_ticks) -> int | None`. The candle resting-order rule
  on one trade price: a stop (`"stop"` or `"limit"` only) fills at the trade when it is at or beyond
  its level; a limit fills at its level once a trade is strictly beyond it. An equal print does not
  fill a limit.
- `fill_cost(*, quantity_steps, volume_step, price_ticks, tick_size, point_value, cost_per_contract,
  cost_bps, currency_decimals) -> str`. Quantity is `quantity_steps * volume_step` contracts and price
  is `price_ticks * tick_size`. The fee is `quantity * cost_per_contract +
  |quantity * price * point_value| * cost_bps / 10000`. Every product and sum is exact; the total is
  rounded once, half-to-even, to `currency_decimals` places (0 to 28), and returned padded to that
  many places (for example `"0.0420"`).
- `price_to_ticks(*, price, tick_size) -> int` converts a decimal price to ticks. It rejects a price
  that is not an exact multiple of `tick_size`.

Errors: invalid values (bad side, kind, quote, level, sign, or a fill that slippage makes
nonpositive; a non-exact decimal string; a price off the grid) raise `ValueError`. Results or
integers outside the native range, including arithmetic that Decimal cannot represent exactly, raise
`OverflowError`. Wrong Python types raise `TypeError`. Each function is pure: repeated calls with the
same arguments return the same result.

Model differences are explicit and not unified. `match_quote` is the quote model and `match_trade` is
the trade model. With ask and bid at 100, a buy limit at 100 fills under the quote model; a trade
printing at 100 does not fill it under the trade model.

Shared helpers keep the existing paths unchanged. The candle kernel's resting-order fill and the tick
kernel's exit and entry comparisons call the same helpers, and their golden tests are unchanged. The
research fee `candle::config::side_cost` stays an unrounded f64 model, so `fill_cost` is a separate
exact currency fee. A test checks that the two agree within float precision.

## Crate Responsibilities

### Intrabar Breakeven Stop

For `run_candle`, an enabled `breakeven_trigger_pct` together with
`breakeven_emergency_stop_ticks` selects tick-ordered breakeven execution. The backend
supplies **100 ticks** by default; `0` disables the emergency stop while keeping intrabar
activation and breakeven fills. A positive emergency distance also requires a finite positive
instrument `tick_size`. It is a price increment, independent of financial `point_value`.

Before activation the stop is entry minus ticks × tick size for a long, or entry plus that
distance for a short. The first trade price reaching the favorable percentage threshold arms
breakeven, replacing the emergency stop with entry ± `breakeven_offset_pct`. Each stop fills
at the first observed trade price at or beyond it, with the tick timestamp and reason
`breakeven_emergency` or `breakeven`. Gaps can exceed the stop level. Activation is never
inferred from a later candle extreme. Priced entries ignore prices before and at their entry
touch; queued entries start protection after their opening print. Explicit protective orders
take precedence at a shared tick, followed by breakeven.

An intrabar source and open/high/low columns are mandatory in this mode. Source failures abort
the run. Omitting the new emergency parameter from a low-level call retains the historical
bar-based ABI for archived parity fixtures; current backend runs always supply it for an
enabled breakeven rule. The Python binding advertises `engine.BREAKEVEN_INTRABAR`.

Every responsibility named in §5 of the system architecture is mapped to exactly one crate:

| Crate | Responsibility | Role & Boundary |
| --- | --- | --- |
| `q-indicators` | Indicator mathematics | Pure technical indicators, transforms, streaming chart studies, and market context kernels: `realized_vol`, `yang_zhang`, `rsi`, `bollinger_bands`, `macd`, `donchian_channels`, `atr`, `sma`, `ema`, `smma`, `wma`, `hma`, `session_vwap`, `rolling_zscore`, `rolling_rank`, `pct_change`, `clip`, streaming states (`SmaState`, `EmaState`, `BollingerState`, `RsiState`, `AtrState`, `SessionVwapState`, `VolumeState`), and market context analysis (`ContextState`, `batch_context`). Zero I/O, zero system clock or environment access. |
| `q-engine` | Simulation & execution kernels | Candle and tick kernels, session tick slicing and OHLC checks through a loader trait, fill model, exit-rule state machines, position sizing. Shared identically by backtesting and live evaluation. |
| `q-buffers` | Columnar memory buffers & contracts | Columnar ring buffers, streaming bar windows, Arrow memory layouts, and vendored contract types (`contracts/`). |
| `q-io` | Columnar codecs & readers | Parquet and Arrow codecs, bar readers, and filtered/sorted session trade-price readers. Reads explicitly provided files/buffers; never performs filesystem discovery. |
| `q-py` | Python native binding | PyO3 projection layer. Exposes `q_core` module, callables, and the native session tick source to Python. Computational semantics remain in Rust semantic crates. |
| `q-qt` | Qt native binding | CXX-Qt projection layer. Exposes `CoreInfo` and QObject interfaces to Qt/QML. Projection only; contains zero computational semantics. |
| `q-parity` | Parity gate & reference fixtures | Test support for reference fixtures, numerical tolerance comparisons, double-run determinism, and prefix causality. `publish = false`; dev-dependency only. |

---

## Dependency Direction

Dependencies flow strictly downward and are enforced by Cargo manifests:

```
q-indicators  ───►  (none)            [dev: q-parity]
q-buffers     ───►  contracts module
q-io          ───►  q-buffers
q-engine      ───►  q-indicators, q-buffers
q-py          ───►  q-engine, q-io, q-buffers, q-indicators
q-qt          ───►  q-engine, q-io, q-buffers, q-indicators
q-parity      ───►  (none)
```

- Any attempt to introduce an upward or cyclic dependency (such as `q-indicators -> q-engine`) fails the build at `cargo metadata` / compile time.
- Neither binding crate (`q-py`, `q-qt`) depends on the other. Building `q-py` requires no Qt toolchain; building `q-qt` requires no Python headers.
- Neither binding crate depends on `q-parity` (enforced by `make parity-isolation`). `q-parity` is used strictly as a dev-dependency.
- No semantic crate depends on a host or knows whether it runs in Python, Qt, or unit tests.

---

## Determinism & Purity Guards

1. **`#![forbid(unsafe_code)]`**:
   Enforced in all four semantic crates (`q-indicators`, `q-engine`, `q-buffers`, `q-io`). Unsafe code is only permitted in the two binding crates (`q-py`, `q-qt`) where FFI boundaries require it.
2. **Disallowed Non-Deterministic APIs**:
   The shared workspace Clippy configuration denies `std::time::Instant`, `std::time::SystemTime`, `std::time::Instant::now`, `std::time::SystemTime::now`, `std::env::var`, and `std::env::var_os`.
3. **Floating-Point Comparison**:
   Direct floating-point equality (`==` and `!=`) is denied by `clippy::float_cmp` and `clippy::float_cmp_const`.

---

## Research indicator kernels (`q-indicators`)

Batch kernels for research features live beside the pandas-parity set. They take `&[f64]` columns (plus `int64` tick metadata for quotes) and return `Vec<f64>` of the same length; NaN is the only missing value. Natural logarithms require finite, strictly positive prices. Per-bar volatility is in log-return units and is never annualized; `yang_zhang(..., periods_per_year = 1)` is the per-bar Yang–Zhang estimator. Volatility at or below `1e-12` is a valid output and an invalid denominator for quotients. Half-life `H` uses `lambda = exp(-ln 2 / H)`; exponentially weighted kernels seed on the first valid input and stay NaN until `N_EW(H) = 1 + ceil(H * log2(100))` samples have been seen, then restart after any invalid price. A half-life above `5e8` is rejected, so `N_EW` always fits 32 bits.

| Kernel | Role |
| --- | --- |
| `log_return` | `ln(C_t) - ln(C_{t-h})` |
| `realized_vol_per_bar` | `sqrt(mean(r^2))` over a trailing window (mean not removed) |
| `ewma_vol_per_bar` | `sqrt` of EWMA of squared log returns |
| `rogers_satchell` | Rogers–Satchell range estimator per bar |
| `true_range` | Classic true range from high, low, and previous close |
| `atr_fraction` | `atr / close` |
| `price_zscore_log` | Z-score of log closes (sample std, `ddof = 1`) |
| `ewma_trend_spread` | Fast minus slow EWMA of log closes (`fast < slow`) |
| `vol_normalize` | `values / (sigma * sqrt(horizon))` with `vol_lag` 0 or 1 |
| `volatility_log_ratio` | `ln(sigma_short) - ln(sigma_long)` |
| `quote_spread_bars` | Relative bid–ask spread aggregated to `[open, close)` bars |
| `volume_at_price` | Volume profile per caller-defined `[open, close)` segment with POC and value area |
| `volume_at_price_levels` | Developing POC, VAH and VAL at checkpoints without lookahead |

`volume_at_price` and `volume_at_price_levels` read a trade tape (`time_msc`, `price`, `volume`, `flags`) in source order. A tick is a trade when `flags` includes `TICK_FLAG_BUY` or `TICK_FLAG_SELL` (`1 << 5` / `1 << 6`); exactly one side bit classifies the trade, both bits mean unknown, and other ticks are ignored. Non-finite or non-positive price or volume on a flagged trade is skipped and counted in `skipped_trades`. With tick size `t` and `row_ticks` `m`, the tick index is `round_half_even(price / t)`; prices more than `1e-6` ticks off the grid are counted in `off_grid_trades` but binned to the nearest tick. Row index `r = floor(k / m)` sets the row price to `r * m * t`. A finite positive price whose rounded tick index does not fit in int64 is rejected with `IndicatorError` rather than saturated. Row and profile totals accumulate trades in source order, including across aggressor sides. Point of control is the row with the largest total; ties go to the row nearest the centre of the traded row range, then to the lower row. Value area expands from the POC row by repeatedly adding the larger adjacent row inside the traded range (both when equal, the remaining side when one is exhausted) until its volume reaches `value_area × total`; gap rows count as zero. A share of exactly `1` always covers the complete traded range, including rows whose volume is lost to floating-point rounding in the total. `volume_at_price(..., segment_open_msc, segment_close_msc, tick_size, row_ticks, value_area)` returns two dicts (`rows`, `segments`) with the column names in the task spec; ticks at a segment close belong to the next segment. `volume_at_price_levels(..., session, checkpoint_msc, checkpoint_session, ...)` returns one dict (`poc`, `vah`, `val`, `total`) from trades of `checkpoint_session` with time strictly before each checkpoint; the profile resets on session change.

`quote_spread_bars(time_msc, bid, ask, flags, session, bar_open_msc, bar_close_msc, max_quote_age_ms)` returns `(last, time_weighted_mean, max, coverage)`. Spread is `(ask - bid) / mid`. Quote updates are ticks whose MQL5 flags include bid (`2`) or ask (`4`). A quote is valid when `0 < bid <= ask`; invalid updates clear the carry. A valid quote is carried until the next quote update, a session change on a tick, or `max_quote_age_ms` after its update (expiry needs no tick). A quote adds duration over `[update, expiry)`. A tick before a bar's open — before the first bar or in a gap between bars — only updates the carried quote: the quote counts from the bar's open and never for time outside the bar, so `coverage` stays within `[0, 1]`. `last` is the quote still carried at the bar's close; a quote that expired before the close gives NaN. `time_weighted_mean` and `max` weight positive-duration intervals inside each bar; `coverage` is carried duration divided by bar length. Ticks at a bar’s close belong to the next bar. Python exposes every kernel under `q_core.indicators`.

---

## Market Context Analysis (`q-indicators::context`)

`q-indicators::context` exposes deterministic, inspectable market-context evaluations by composing streaming price studies (`EmaState`, `RsiState`, `AtrState`, `SmaState`, and `SessionVwapState`). It provides both incremental state evaluation (`ContextState`) and slice evaluation (`batch_context`).

### Defaults (`ContextConfig`)

| Parameter | Default | Validation Rule |
| --- | --- | --- |
| `fast_period` | 9 | `1..=1000`, `fast_period < slow_period` |
| `slow_period` | 21 | `1..=1000` |
| `rsi_period` | 14 | `1..=1000` |
| `rsi_lower` / `rsi_upper` | 30.0 / 70.0 | finite, `0 < rsi_lower < rsi_upper < 100` |
| `atr_period` | 14 | `1..=1000` |
| `volatility_baseline_period` | 20 | `1..=1000` |
| `volatility_lower` / `volatility_upper` | 0.8 / 1.2 | finite, `0 < volatility_lower < volatility_upper` |
| `vwap_extension` | 1.0 (ATR multiples) | finite, `> 0` |

### Category Semantics & Evidence

- **Trend (`TrendReading`)**:
  - `Upward`: Fast EMA > Slow EMA and both one-bar slopes > 0.
  - `Downward`: Fast EMA < Slow EMA and both one-bar slopes < 0.
  - `Balanced`: Fast EMA == Slow EMA and both one-bar slopes == 0.
  - `Mixed`: All other valid conditions.
  - Requires 1 prior committed bar for slope; absent ATR leaves `normalized_spread` `None` without invalidating the reading.
- **Momentum (`MomentumReading`)**:
  - `LowerZone`: RSI < `rsi_lower`.
  - `MiddleZone`: `rsi_lower <= RSI <= rsi_upper` (threshold equality is middle).
  - `UpperZone`: RSI > `rsi_upper`.
  - Change relative to preceding committed bar is labeled `Rising`, `Falling`, or `Unchanged`.
- **Volatility (`VolatilityReading`)**:
  - Ratio = ATR / SMA(ATR, `volatility_baseline_period`).
  - `Contracting`: Ratio < `volatility_lower`.
  - `Typical`: `volatility_lower <= Ratio <= volatility_upper` (threshold equality is typical).
  - `Expanding`: Ratio > `volatility_upper`.
  - Nonpositive or nonfinite ATR baseline SMA is rejected as `Unavailable` with stated reason.
- **VWAP (`VwapReading`)**:
  - Signed distance = (Close - VWAP) / ATR.
  - Position: `Above`, `Below`, or `At` VWAP (independent of distance availability).
  - Extension: `Extended` (`|distance| >= vwap_extension`), otherwise `Near`.
  - Zero ATR leaves distance `None` while position remains available; disabled VWAP or missing volume affects only VWAP status.

### Lifecycle & Invariance

`ContextState` exposes `commit`, non-mutating `preview`, and `reset`:
- **Preview Equivalence**: `preview(input)` on state $S$ produces bitwise identical readings to `commit(input)` on state $S$.
- **Repeated Preview Invariance**: Repeated provisional updates never mutate internal state or induce preview drift.
- **Input Isolation**: Invalid config or malformed input is rejected prior to state advancement, preserving existing state bitwise.

---

## Contracts Vendoring Protocol

`q_core` vendors generated Rust types from `q_contracts`:
- `CONTRACTS_REV`: Commit hash of `q_contracts` pinned by this workspace.
- `contracts/`: Vendored generated Rust types (`api.rs`, `catalog.rs`, `edge.rs`, `mod.rs`, `stream.rs`), included by `q-buffers`.
- `make contracts`: Clones `q_contracts` at `CONTRACTS_REV` and copies generated Rust files.
- `make contracts-check`: Clones `q_contracts` at `CONTRACTS_REV`, regenerates Rust contracts from schemas, and asserts zero diff.

---

## Reference Fixtures & Parity Gate

`q_core` gates parity, determinism, and causality of its numerical kernels against reference fixtures generated from `q_backend`:
- `BACKEND_REV`: Commit hash of `q_backend` pinned by this workspace for reference fixtures.
- `fixtures/reference/`: Committed reference datasets (`inputs/`), golden outputs (`indicators/`), and backend-family fixtures (`bar_window/`, `exit_rules/`, `candle_engine/`, `decision_step/`, `tick_kernel/`, `tick_bars/`).
- `make fixtures`: Exports numeric-family fixtures from `q_backend` at `BACKEND_REV` using the pinned exporter environment (`tools/reference`).
- `make fixtures-check`: Regenerates numeric families into a temporary directory and compares those subtrees (not backend families).
- `make fixtures-backend` / `make fixtures-backend-check`: Export / verify backend families (`BACKEND_FAMILIES`, currently `bar_window`, `exit_rules`, `candle_engine`, `decision_step`, `tick_kernel`, and `tick_bars`) that require the full `q_backend` locked environment. Required locally before merging changes to those families' exporters, their fixture subtrees, or `BACKEND_REV`. Not run in CI: the sync pulls multi-GB wheels (torch / nvidia-*), while every `cargo test` already fail-closes on a pin edit without regenerated fixtures via `ProvenanceRev`.
- `make parity-isolation`: Confirms that `q-parity` is not included in the normal or build dependency tree of `q-py` or `q-qt`.
- `make bench-bar-window`: Human measurement of `RollingBarWindow` vs pandas window ingest cost (not part of `make check`).
- `make bench-candle-kernel`: Human measurement of `q_core.engine.run_candle` vs `BacktestEngine._run_single_chunk` on a 50_000-bar scripted series (not part of `make check`). Set `Q_BACKEND_CHECKOUT` to reuse an existing backend tree.
- `make bench-tick-kernel`: Human measurement of the Rust tick kernel vs the pinned numba `simulate` on a 5,000,000-tick stream (not part of `make check`).

### Pending Fixtures and Kernel Implementation

`crates/q-indicators/tests/reference_pending.txt` tracks all reference functions that are accounted for by the reference suite but not yet bound to a Rust kernel. The parity gate fail-closes on any unaccounted, mismatched, or conflicting fixture.

**Procedure for implementing a kernel (Q-022+):**
1. Implement the kernel in `q-indicators`.
2. Add a `Binding` to `BINDINGS` in `crates/q-indicators/tests/reference_gate.rs`.
3. Delete the corresponding function id line from `crates/q-indicators/tests/reference_pending.txt`.
4. Run `cargo test -p q-indicators --test reference_gate -- --nocapture` to verify that golden outputs, double-run determinism, and prefix causality all pass.

---

## Prerequisites

Building and testing `q_core` requires:
- **Rust Toolchain**: 1.98.0 (pinned via `rust-toolchain.toml`), with `rustfmt` and `clippy`.
- **C++ Compiler & Build Tools**: `g++` (C++17 support), `make`, `git`, `curl` (`build-essential` on Debian/Ubuntu).
- **Python Toolchain**: Python 3.12+ and `uv` (for maturin release wheel builds and isolated virtualenv integration tests).
- **Qt 6 Runtime Libraries**: If a system Qt 6 SDK is not installed, `qt-build-utils` / `cxx-qt` automatically downloads a minimal Qt 6 core runtime (`qt_minimal`). The downloaded runtime dynamically links:
  - On Debian/Ubuntu: `libglib2.0-0t64` (or `libglib2.0-0`), `libdouble-conversion3`, and `libpcre2-16-0`.

---

## Development and Validation

The canonical check command runs format verification, linting, tests, wheel building, wheel integration test, Qt harness test, and contracts check:

```bash
make check
```

Individual targets:
```bash
make fmt-check        # Verify rustfmt formatting
make lint             # Run clippy with -D warnings
make test             # Run cargo test across workspace
make wheel            # Build release wheel with maturin
make wheel-test       # Test installing and importing wheel in clean venv
make qt-test          # Build q-qt static library and run C++ test harness
make contracts-check  # Verify vendored contracts match clean regeneration
```

### Local CI and Git hooks

`make ci` (or `./scripts/ci.sh`) runs the same targets as `make check`, cheapest first, after a
prerequisite preflight that names any missing tool or Qt runtime library. Locally it enters the host
user `ci.slice` when available, lowers CPU/IO priority, and caps Cargo at half the logical CPUs
(override with `CARGO_BUILD_JOBS=N`); with `CI` set it uses Cargo's defaults. Do not wrap the
command in `systemd-run`.

Install the Git hooks once per clone:

```bash
make hooks
```

- `pre-commit`: `make fmt-check`, plus `make lint` when Rust or Cargo files are staged.
- `pre-push`: the full `./scripts/ci.sh` pipeline.
