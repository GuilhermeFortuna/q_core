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
`(entry, exit_long, exit_short, strength, stop_price, target_price)` to give an entry queued
on this bar its own levels (`NaN` means no level; without the two extra values the static
`stop_price`/`target_price` columns apply). Entry is `-1`, `0`, or `1`, exit flags are booleans,
strength must be finite and within `[0, 1]`, and levels must be `NaN` or finite and positive.
Runtime levels need the `intrabar` source, as static ones do. Invalid decisions or callback
exceptions abort the run. Callback runs own copies of input arrays so host mutations cannot
change the simulation. Calls without these arguments retain the static signal path. This native
extension requires no contract or Qt API changes.

Custom intrabar exits are requested with `exit_screen_callback` and `exit_tick_callback`,
which must be passed together and require `intrabar`. The screen receives
`(bar, positions)` and returns a bool; it is called once per candle with open positions, after
queued fills and before intrabar processing, and flat candles are never screened. A `True` screen
loads that candle's trade prices. The tick receives `(bar, tick, time_us, price, positions)` for
each price in order and returns a bool; `True` closes every open trade at that traded price,
except a trade opened on this same tick. A screen is conservative: a false positive costs a
replay but never fills. Rust callers use `q_engine::run_candle_with_callbacks` with
`RuntimeCallbacks { strategy, exit }`.

## Intrabar Protective Orders (`q-engine::candle`)

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
   when some open trade's level passes the screen or the custom exit screen qualifies, and at most
   once per bar.
3. **Walk.** The bar's trade prices are walked once, in order. At each price, screened trades are
   checked for their stop (first, then target) and a stop fills at the traded price, so a bar that
   opens past the level fills at its open. A target fills at its level once a price trades strictly
   through it; a price equal to the target does not fill. A trade opened on this bar skips its first
   price, which is its own fill. A custom tick exit at the same price is evaluated only after the
   protective checks, so a protective fill takes precedence; the first executable event in price
   order wins, and the walk stops once no positions remain open.
4. **Close.** The trade closes at the triggering fill price, charged its exit-side cost, and its
   time is recorded in `exit_time_us`. A screen pass with no triggering price leaves the trade open.
   A source that returns no prices, or fails, ends the run with `CandleError::IntrabarSource` naming
   the bar.

Section D sees a protected trade as closed. Without `protective`, every output is unchanged.

## Crate Responsibilities

Every responsibility named in §5 of the system architecture is mapped to exactly one crate:

| Crate | Responsibility | Role & Boundary |
| --- | --- | --- |
| `q-indicators` | Indicator mathematics | Pure technical indicators, transforms, streaming chart studies, and market context kernels: `realized_vol`, `yang_zhang`, `rsi`, `bollinger_bands`, `macd`, `donchian_channels`, `atr`, `sma`, `ema`, `smma`, `wma`, `hma`, `session_vwap`, `rolling_zscore`, `rolling_rank`, `pct_change`, `clip`, streaming states (`SmaState`, `EmaState`, `BollingerState`, `RsiState`, `AtrState`, `SessionVwapState`, `VolumeState`), and market context analysis (`ContextState`, `batch_context`). Zero I/O, zero system clock or environment access. |
| `q-engine` | Simulation & execution kernels | Candle and tick kernels, fill model, exit-rule state machines, position sizing. Shared identically by backtesting and live evaluation. |
| `q-buffers` | Columnar memory buffers & contracts | Columnar ring buffers, streaming bar windows, Arrow memory layouts, and vendored contract types (`contracts/`). |
| `q-io` | Columnar codecs & readers | Parquet and Arrow codecs and byte decoders. Reads explicitly provided files/buffers; never performs filesystem discovery. |
| `q-py` | Python native binding | PyO3 projection layer. Exposes `q_core` module and callables to Python. Projection only; contains zero computational semantics. |
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
