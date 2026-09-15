# Q-027 implementation plan: Candle kernel

**Status:** authoritative in the [Q project board](https://github.com/users/GuilhermeFortuna/projects/2)  
**Specification:** [`../specs/Q-027-candle-kernel-spec.md`](../specs/Q-027-candle-kernel-spec.md)  
**Depends on:** Q-024, Q-026

## Current-system context

After Q-026, `q-engine` has `exits` with `ExitParams`, `ParamValue`,
`ExitRuleSet::required_columns`, `ExitInputs::from_slices`, and `ExitBook::evaluate`
keyed by `PositionKey(u64)`, all gated by the `exit_rules` backend family at
`BACKEND_REV` `067e29c`. `q-py` projects indicators (`src/indicators.rs`, the
`contiguous()` helper rejects non-contiguous or wrongly typed arrays with a
`ValueError` naming the argument) and frames (`src/frame.rs`), and has no engine
module. `tests/test_wheel.py` and `tests/test_bar_frame.py` run through
`make wheel-test`; `tests/bench_bar_window.py` behind `make bench-bar-window` is
the pattern for a human benchmark.

In `q_backend`, `BacktestEngine._run_single_chunk` (`backtesting/engine.py:114`
after Q-023, `:113` at the pin) computes `fill_price = row.get("open",
row.get("close", 0.0))` and runs, per bar: A (force close at `current_time >=
close_t`, lines 169-180), B (each queued `CLOSE` closes every open trade whose
symbol matches, reason `sig.exit_reason or "SIGNAL"`, lines 182-192), C
(`sizer.size_signal(sig, fill_price, capital, current_data=row)`, then
`max_position_size`, `remaining = max_size - sum(open qty on symbol)`, skip at
`<= 0`, trim, `Trade(..., commission=side_cost(...))`, lines 194-220), D
(queue exits and entry, with day-trade gating, lines 222-259), E (last bar of
day closes at `row.get("close", fill_price)`, lines 261-272), and after the loop
`force_close_at_end` closes at the final row's close with `FORCE_CLOSE`
(lines 274-281). `_close_trade_with_costs` adds the exit `side_cost` before
`TradeRegistry.close_trade` (`registry.py:24`) sets
`pnl = (exit - entry) * qty * point_value` (sign by side) `- commission`, and the
engine adds `pnl` to capital. `side_cost` (`costs.py:11`) returns exactly 0.0
without a config, else `quantity * cost_per_contract + (cost_bps / 10_000) *
price * quantity * point_value`. The three sizers are in `position_sizing.py`:
`FixedQuantitySizer` floors `quantity * strength` and caps at `quantity`;
`FixedSafetyMarginSizer._target_contracts` (line 175) floors `capital / margin`,
applies max, then the min-contracts rule, and floors again after strength;
`InverseVolatilitySizer` reads `row["volatility"]`, floors
`(target / 100.0) * capital / (vol * price * point_value)`, clamps, and its
`max_position_size` falls back to `max_contracts` or `None` (line 323).
`DAY_TRADE` mode (`run`, line 98) groups by `index.date` and calls
`_run_single_chunk(chunk, force_close_at_end=True)` per day with fresh capital.
After Q-024, section D is one call to
`signal_columns.evaluate_queued_signals(strategy, signals, position, row, open_trades)`:
`exit_strategy.check_exits`, then one `CLOSE` per open trade not on a
rule-closed symbol whose side matches `exit_long`/`exit_short` (or whose
`bar_index` distance reaches `holding_period_bars`), then at most one entry from
`q_signal_entry`/`q_signal_strength`. At the pin, the same order is
`execution/signal_eval.evaluate_queued_signals` over row methods, and
`execution/parity.reference_queued_signals_by_close` (line 43) drives it per bar
for `test_backtest_live_parity`. The gap is that the loop, the sizers and the
decision order exist only as Python over row objects, and no fixture pins the
engine's ledger outside the thirteen goldens.

## Interfaces produced

```rust
// crates/q-engine/src/lib.rs   (changed)
pub mod candle;
pub use candle::{run_candle, CandleConfig, CandleError, CandleInputs, CandleRun, Costs,
                 DayTradeWindow, Decision, DecisionStep, DecisionTrace, ExitReason, QueuedEntry,
                 QueuedExit, SignalColumns, Sizing, TradeLedger, TradeView};
```

```rust
// crates/q-engine/src/candle/sizing.rs   (new)
#[derive(Clone, Debug)]
pub enum Sizing {
    FixedQuantity { quantity: f64, scale_by_strength: bool },
    FixedSafetyMargin { margin_per_contract: f64, min_contracts: i64,
                        max_contracts: Option<i64>, scale_by_strength: bool },
    InverseVolatility { target_volatility_pct: f64, point_value: f64, min_contracts: i64,
                        max_contracts: Option<i64>, scale_by_strength: bool },
}
impl Sizing {
    pub fn validate(&self) -> Result<(), CandleError>;   // the sizers' __init__ checks, same order
    /// size_signal for a BUY/SELL; None = no order. `volatility` is the fill row's value, None if absent.
    pub fn size(&self, strength: f64, price: f64, capital: f64, volatility: Option<f64>) -> Option<f64>;
    pub fn max_position(&self, price: f64, capital: f64) -> Option<f64>;   // None = unbounded
}
```

```rust
// crates/q-engine/src/candle/config.rs   (new)
#[derive(Clone, Copy, Debug)]
pub struct Costs { pub per_contract: f64, pub bps: f64 }
pub(crate) fn side_cost(costs: Option<Costs>, price: f64, quantity: f64, point_value: f64) -> f64;

/// Microseconds since wall-clock midnight; parsed from "HH:MM" by the caller.
#[derive(Clone, Copy, Debug)]
pub struct DayTradeWindow { pub entry_start_us: i64, pub entry_end_us: i64, pub force_close_us: i64 }

#[derive(Clone, Debug)]
pub struct CandleConfig {
    pub initial_capital: f64,
    pub point_value: f64,                   // the engine's point_values.get(symbol, 1.0)
    pub costs: Option<Costs>,
    pub sizing: Sizing,
    pub holding_period_bars: Option<i64>,
    pub exit_params: ExitParams,
    pub day_trade: Option<DayTradeWindow>,  // None = day_trade False
    pub force_close_at_end: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum CandleError {
    InvalidConfig { field: &'static str, reason: String },
    LengthMismatch { column: String, expected: usize, actual: usize },
    InvalidSignal { column: &'static str, bar: usize, reason: &'static str },
    Exit(ExitError),
}
```

```rust
// crates/q-engine/src/candle/inputs.rs   (new)
/// Q-024's decision columns.
pub struct SignalColumns<'a> {
    pub entry: &'a [i8],                    // +1 BUY, -1 SELL, 0 none
    pub exit_long: &'a [bool],
    pub exit_short: &'a [bool],
    pub strength: &'a [f64],                // [0, 1] where entry != 0
    pub bar_index: Option<&'a [i64]>,       // required iff holding_period_bars is Some
}

pub struct CandleInputs<'a> {
    pub time_us: &'a [i64],                 // wall-clock microseconds, any order, repeats allowed
    pub open: Option<&'a [f64]>,
    pub high: Option<&'a [f64]>,
    pub low: Option<&'a [f64]>,
    pub close: Option<&'a [f64]>,
    pub signals: SignalColumns<'a>,
    pub volatility: Option<&'a [f64]>,
    pub tradable: Option<&'a [bool]>,       // false = before trade_start: bar is skipped entirely
    pub columns: &'a dyn Fn(&str) -> Option<&'a [f64]>,   // exit-rule indicator columns by name
}
```

```rust
// crates/q-engine/src/candle/decision.rs   (new)
#[derive(Clone, Copy, Debug)]
pub struct TradeView { pub key: PositionKey, pub side: Side, pub entry_price: f64,
                       pub entry_bar: Option<usize> }   // None = entry time is not a bar of the series

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct QueuedExit { pub key: PositionKey, pub rule: Option<ExitRuleId> }   // None = strategy or holding exit
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct QueuedEntry { pub side: Side, pub strength: f64 }
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Decision { pub exits: Vec<QueuedExit>, pub entry: Option<QueuedEntry> }

/// Section D for one closed bar, with exit-rule state that persists across calls.
pub struct DecisionStep { /* book: ExitBook */ }
impl DecisionStep {
    pub fn new(exit_params: ExitParams) -> Self;
    pub fn required_columns(&self) -> Vec<String>;
    /// `holding_period_bars` is the strategy's declaration, passed per call; state is exit-rule state only.
    pub fn decide(&mut self, bar: usize, trades: &[TradeView], signals: &SignalColumns<'_>,
                  exits: &ExitInputs<'_>, holding_period_bars: Option<i64>) -> Decision;
    pub fn state(&self, key: PositionKey) -> Option<&RuleState>;
}
```

```rust
// crates/q-engine/src/candle/run.rs   (new)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExitReason { Rule(ExitRuleId), Signal, EndOfDay, ForceClose }
impl ExitReason {
    pub fn code(self) -> i64;               // rule discriminant 0..=10, Signal 11, EndOfDay 12, ForceClose 13
    pub fn as_str(self) -> &'static str;    // rule id, "SIGNAL", "END_OF_DAY", "FORCE_CLOSE"
}

/// One row per trade in the order trades were opened.
#[derive(Clone, Debug, Default)]
pub struct TradeLedger {
    pub entry_bar: Vec<i64>, pub exit_bar: Vec<i64>,          // exit_bar -1 = open
    pub side: Vec<i8>, pub quantity: Vec<f64>,
    pub entry_price: Vec<f64>, pub exit_price: Vec<f64>,      // NaN = open
    pub commission: Vec<f64>, pub pnl: Vec<f64>,              // pnl NaN = open
    pub exit_reason: Vec<i64>,                                // ExitReason::code, -1 = open
}

/// Per bar: exits[offsets[i]..offsets[i+1]] and the entry; bars that were skipped queue nothing.
#[derive(Clone, Debug, Default)]
pub struct DecisionTrace {
    pub exit_offsets: Vec<i64>,             // len n + 1
    pub exit_reason: Vec<i64>,              // rule code, or 11 for no rule reason
    pub entry: Vec<i8>, pub entry_strength: Vec<f64>,         // 0 and 0.0 when no entry
}

pub struct CandleRun { pub trades: TradeLedger, pub trace: DecisionTrace }
pub fn run_candle(inputs: &CandleInputs<'_>, config: &CandleConfig) -> Result<CandleRun, CandleError>;
```

```python
# crates/q-py/src/engine.rs   (new) -> module q_core.engine
def required_columns(exit_params: Mapping[str, object]) -> list[str]: ...
def enabled_rules(exit_params: Mapping[str, object]) -> list[str]: ...   # rule ids in registry order
def size_entry(sizing: Mapping[str, object], *, point_value: float, strength: float, price: float,
               capital: float, volatility: float | None) -> float | None: ...
def max_position(sizing: Mapping[str, object], *, point_value: float, price: float,
                 capital: float) -> float | None: ...
def run_candle(*, time_us: NDArray[int64], open: NDArray[float64] | None, high: NDArray[float64] | None,
               low: NDArray[float64] | None, close: NDArray[float64] | None,
               entry: NDArray[int8], exit_long: NDArray[bool_], exit_short: NDArray[bool_],
               strength: NDArray[float64], bar_index: NDArray[int64] | None,
               volatility: NDArray[float64] | None, tradable: NDArray[bool_] | None,
               columns: Mapping[str, NDArray[float64]], initial_capital: float, point_value: float,
               costs: tuple[float, float] | None, sizing: Mapping[str, object], sizing_point_value: float,
               holding_period_bars: int | None, exit_params: Mapping[str, object],
               day_trade_us: tuple[int, int, int] | None, force_close_at_end: bool) -> dict[str, NDArray]: ...
               # keys: TradeLedger field names, "exit_reason_text" (list[str | None]), DecisionTrace field names

class DecisionStep:
    def __init__(self, *, exit_params: Mapping[str, object]) -> None: ...
    def decide(self, *, bar: int, trades: Sequence[tuple[str, int, float, int | None]],  # (id, +1/-1, entry price, entry bar)
               close: NDArray[float64] | None, high: NDArray[float64] | None, low: NDArray[float64] | None,
               entry: NDArray[int8], exit_long: NDArray[bool_], exit_short: NDArray[bool_],
               strength: NDArray[float64], bar_index: NDArray[int64] | None,
               columns: Mapping[str, NDArray[float64]], holding_period_bars: int | None = None
               ) -> tuple[list[tuple[str, str | None]], tuple[int, float] | None]: ...
    def state(self, trade_id: str) -> dict[str, dict[str, float | int | bool | None]] | None: ...
        # per enabled rule id, the backend's state-dict keys ("extreme", "peak", "armed", "sar", "ep",
        # "af", "prior_low", ..., "ratchet", "bars"); keys the Python rule would not have set are absent
```

```
crates/q-engine/tests/candle_engine_gate.rs     new: engine fixtures, negative controls, determinism, prefix trace
crates/q-engine/tests/decision_step_gate.rs     new: queued-signal parity fixtures through DecisionStep
tools/reference/families/candle_engine.py       new: backend family driving BacktestEngine._run_single_chunk
tools/reference/families/decision_step.py       new: backend family driving reference_queued_signals_by_close + StrategyEvaluator
tools/reference/families/scripted_strategy.py   new: shared scripted TradingStrategy for both families
fixtures/reference/candle_engine/               new: 17 fixture files
fixtures/reference/decision_step/               new: 7 fixture files
tests/test_engine.py                            new: wheel test for q_core.engine
tests/bench_candle_kernel.py                    new: human benchmark against the pinned Python engine
Makefile                                        BACKEND_FAMILIES += candle_engine decision_step; bench-candle-kernel target; wheel-test runs test_engine.py
README.md                                       fixture protocol and benchmark lines
```

## Implementation decisions

- **The kernel is `_run_single_chunk`, not `run`.** `DAY_TRADE` mode recomputes
  indicators per calendar day, which is Python work over pandas, and resets
  capital per chunk. Keeping the day split in the caller means the kernel never
  needs indicators, and the caller passes `force_close_at_end = true` per day
  exactly as `run` does.

- **Inputs are plain slices, not a Q-025 `BarFrame`.** `BarFrame::try_new`
  requires strictly increasing times and reserves `spread`. The engine accepts any
  index, the goldens and lake frames are sorted but nothing enforces it, and
  `gatev_pairs` writes a float `spread` and overwrites the price columns. Building
  a frame would reject inputs the engine runs today. A caller holding a frame can
  still pass its slices.

- **Times of day and calendar days come from `time_us` with Euclidean
  division by 86,400,000,000.** `timestamp.time()` and `timestamp.date()` in
  the engine read the index's own wall clock with no conversion, and Q-028 passes
  that wall clock. `div_euclid`/`rem_euclid` keep dates correct for times before
  1970, which plain `/` and `%` would not. Comparing microseconds since midnight
  with `HH:MM` boundaries converted to microseconds is the same comparison as
  `time >= datetime.time(h, m)`.

- **`tradable` is a mask, not a start index.** `timestamp < trade_start` is
  evaluated per row, and a start index would assume sorted times. A `false` bar
  runs none of A to E, so nothing can be queued before trade start, as the
  `continue` at the top of the loop does.

- **Trades live in one `Vec` in open order, and "open trades" is a filter over
  it.** `TradeRegistry.trades` is a dict in insertion order and
  `get_open_trades` filters it, so iteration order over open trades, which decides
  exit order and cap sums, is open order. The ledger is that `Vec` projected into
  columns.

- **B closes every open trade on the first queued `CLOSE` and ignores the
  rest.** The Python loop matches trades by symbol, and a strategy has one symbol,
  so the first `CLOSE` closes every trade of both sides with its own reason and
  later ones find nothing open. The kernel applies the first queued exit's reason
  (`Rule(id)`, else `Signal`) to all open trades. Fixture `k04` holds a long and
  a short at once to pin it.

- **`DecisionStep` owns the `ExitBook`, and the loop calls the same `decide` as
  the evaluator.** One implementation of section D is invariant 1 applied to the
  step itself. The holding period is a `decide` argument rather than step state,
  because it is a property of the strategy while the step's state is exit-rule
  state; the backend keeps that state on the strategy's exit-strategy object,
  which is created before the strategy's holding period is known. Day-trade gating stays in the loop around the call: exits are
  decided only when `time < force_close && !last_bar_of_day`, and the entry is
  kept only inside the inclusive window, because that is engine policy, not a
  decision. The step always runs exit-rule updates when called, so gating by not
  calling it reproduces the engine's skipped `check_exits` on gated bars.

- **Holding period uses `bar_index` values, not positions.** Q-024's consumer
  closes when `bar_index[position] - bar_index[entry] >= holding_period_bars`.
  `bar_index` is frame-relative and equals the position in every frame today, but
  the rule is stated on the column, and reading positions would silently diverge
  if a strategy ever offsets it. In the loop a trade's `entry_bar` is its fill
  bar. The evaluator supplies `None` when `opened_at` is not a bar time.

- **Decision columns are validated once per run.** `entry` outside `{-1, 0, 1}`,
  NaN or out-of-range strength on an entry bar, a true exit column while a holding
  period is declared, and a missing `bar_index` with a holding period are
  `InvalidSignal` before the loop. Q-024's validator rejects the same things in
  Python, so this never fires on backend input, but the kernel must not read an
  undefined decision from another caller.

- **Contract counts are computed in `f64` with `floor`, never cast to `i64`
  mid-formula.** Python's `math.floor` returns an unbounded int that is later
  converted with `float()`. Every count the sizers produce is an exact integer far
  below 2^53, where `f64` is exact, and a cast would saturate or truncate
  differently for negative capital. Min and max contracts are compared as `f64`.

- **Every arithmetic expression keeps Python's association.**
  `((vol * price) * point_value)`, `((target / 100.0) * capital) / notional`,
  `(q * cpc) + ((((bps / 10_000.0) * price) * q) * pv)`,
  `((exit - entry) * qty) * pv - commission`, and commission as
  entry cost then `+=` exit cost. `side_cost` with no config returns the literal
  0.0, not the formula with zeros, because `-0.0` and NaN propagation differ.
  `suboptimal_flops` is allowed with a reason on these functions only, as in
  Q-026.

- **The exporter judges the loop through a scripted strategy at the existing
  pin.** The pin predates Q-024, so real strategies there have row methods, and
  moving the pin would regenerate every family and cross Q-023. A
  `ScriptedStrategy(TradingStrategy)` joins decision arrays to the frame by
  position in `compute_indicators`, returns `BUY`/`SELL` with the scripted
  strength from `check_entry_conditions`, and from `check_exit_conditions` returns
  one `CLOSE` per open trade whose side matches the exit column, or
  `lai_lau_common.fixed_holding_period_exits` when a holding period is scripted.
  That is exactly the decision Q-024 moves into columns. Real strategies are
  judged end to end by Q-028's goldens.

- **The candle-engine family calls `BacktestEngine._run_single_chunk` directly.**
  It is the function the kernel replaces, `force_close_at_end` is a parameter
  there, and calling `run` in `DAY_TRADE` mode would also test pandas date
  grouping, which stays in Python.

- **Engine scenarios (17 files):** over `synthetic_ohlcv` with the stated
  parameters, each with scripted decisions generated from a fixed seed and hand
  edits for the edge being pinned.
  - `k01_strategy_exits` (long and short, fixed quantity);
  - `k02_stop_target_rules` (`fixed_sl`, `fixed_tp`);
  - `k03_atr_trailing_rules` (`atr_sl`, `trailing`, `chandelier`);
  - `k04_long_short_both_open` (a sell entered while a buy is open, then one long
    exit);
  - `k05_rule_and_strategy_same_bar`;
  - `k06_pyramid_cap_trim` (quantity 2 with strength 0.5 and repeated entries);
  - `k07_safety_margin_compounding` (margin 5,000, min 1, max 30);
  - `k08_inverse_volatility` (volatility column with NaN, zero and negative bars,
    max 5, min 1);
  - `k09_strength_floor_to_zero`;
  - `k10_costs` (per contract 1.5 and 3 bps, point value 0.2);
  - `k11_holding_period` (period 7, exit columns true where they would be ignored);
  - `k12_day_trade_sequential` (`day_trade=True` with 09:00, 16:00, 17:00 on 15-minute bars);
  - `k13_day_trade_chunk_force_close` (one day, `force_close_at_end=True`);
  - `k14_trade_start_warmup` (first 50 bars not tradable);
  - `k15_close_only_series` (no open, high or low column);
  - `k16_repeated_times` (two bars sharing a time, no holding period, because
    `_timestamp_to_bar.get` returns a Series on duplicates at the pin);
  - `k17_all_rules_psar_ratchet_breakeven` (the remaining rule families).

  Each case stores inputs, configuration, and the ledger in open order with
  times as bar positions; an exporter assertion maps `Trade.entry_time` back to a
  unique position and fails on ambiguity rather than guessing.

- **The decision-step family records the reference and asserts the evaluator
  agrees at export.** For each scenario it runs
  `reference_queued_signals_by_close` and `StrategyEvaluator.ingest_completed_bars`
  (window bound = series length) with the same scripted strategy and open trade,
  asserts `signals_equal` bar by bar as `test_backtest_live_parity` does, and
  stores the reference's queued exits (reason code or 11) and entry, plus the
  evaluator's `requested_quantity` so `size_entry` at close with initial capital
  is pinned too. Seven scenarios: no exits with no trade, long and short open;
  trailing plus ATR stop with a long open; psar plus time stop with a short open;
  holding period with an on-grid `entry_time`; holding period with an off-grid
  `entry_time`; inverse-volatility sizing.

- **The Python projection interns trade ids per `DecisionStep` in a
  `BTreeMap<String, PositionKey>`.** Keys must be stable across calls so state
  follows `exec-<deployment>` from bar to bar, and ids that disappear are removed
  when the book prunes them, so the map cannot grow without bound.

- **The projection also answers enabled rules, maximum position, and per-trade
  exit state in the backend's dict shape.** Q-031 deletes the Python rules' and
  sizers' per-bar semantics, and the backend's tests assert on
  `ExitStrategy._state`, `rule.is_enabled` and `max_position_size`. Without these
  three read-only answers the backend would have to keep Python copies to test
  against, which is the duplication this batch removes. The state dict uses the
  Python rules' key names and omits keys they would not yet have set, so
  existing assertions keep their meaning.

- **Parameter mappings are coerced by calling Python's `float()` and `int()` on
  each value.** That is the backend's exact coercion, including strings and
  numpy scalars, and it avoids reimplementing it in Rust. The resulting numbers
  go into `ParamValue`.

- **Sizing mappings are the backend's `PositionSizingConfig.model_dump()`
  dicts, with `sizing_point_value` separate.** The engine's trade point value
  comes from `point_values[symbol]`, while `InverseVolatilitySizer` gets
  `build_position_sizer(point_value=...)`. They are separate inputs today and can
  differ, so the kernel keeps both.

- **Day-trade times arrive as microseconds, parsed by the caller.** The engine's
  `HH:MM` parsing raises a specific `ValueError` message that is part of the
  backend's behaviour, so parsing stays in Q-028's Python.

- **Throughput is measured, not asserted.** The kernel should be much faster than
  a pandas row loop, but a timing threshold in `make check` would be flaky across
  machines. The benchmark runs the pinned Python engine and the wheel on the same
  50,000-bar series, five runs each, and reports individual and median figures
  whichever way they fall.

## Ordered implementation

1. Work on the branch `Q-027-candle-kernel` in `q_core`, created from
   `development` by `./work start`. Confirm Q-026 is merged (`q_engine::exits`
   exists and `exit_rules` fixtures pass) and read Q-024's merged
   `backtesting/signal_columns.py` in `q_backend` `development`. If its column
   names, dtypes or holding-period rule differ from this plan's `SignalColumns`,
   stop and report before writing code.
2. Write failing tests in `candle/config.rs`: `side_cost(None, ...)` is `0.0`
   with a positive sign bit; `side_cost(Some(1.5, 3.0), 101.25, 2.0, 0.2)` equals
   bit for bit the value `costs.side_cost` returns in `q_backend` for the same
   arguments (compute it once in Python and record its bits in the test, not a
   decimal literal). Confirm they fail. Implement. Confirm they pass. Commit.
3. Write failing tests in `candle/sizing.rs`:
   - fixed quantity 1.9 opens 1.0; with strength 0.4 and quantity 2 opens nothing;
     `max_position` is 1.9;
   - safety margin 5,000 with capital 12,000 opens 2; with max 1 opens 1; with
     capital 3,000 and min 2 opens 2 (a zero count takes the minimum); with
     capital 7,000 and min 2 opens nothing (a non-zero count below the minimum);
     with capital -1,000 and min 1 opens nothing;
   - inverse volatility with vol 0.25, price 100, point value 0.2, target 10, capital
     100,000 opens `floor(0.1 * 100000 / 5.0)` = 2000, clamped to max 5;
     vol NaN, 0.0, -0.1 and `None` open nothing; `max_position` with max 5 is 5.0
     and without a max is `None`;
   - `validate` rejects margin 0, target 0, point value 0, min -1, and max below
     min, in the sizers' check order.

   Confirm they fail. Implement `Sizing`. Confirm they pass. Commit.
4. Write failing tests in `candle/decision.rs` over hand-built columns:
   - `exit_long[5] = true` with an open long and short gives one exit for the long
     with no rule;
   - with `fixed_sl` firing on the long at bar 5, the output is the rule exit only,
     and the short gets no strategy exit on a short-exit bar;
   - holding period 2 with entry bar 3 exits at bar 5 and not at 4; `entry_bar
     None` never exits; `exit_long` true is ignored while a holding period is set;
   - `entry[7] = -1` with strength 0.25 queues one short with 0.25.

   Confirm they fail. Implement `SignalColumns`, `TradeView` and `DecisionStep`.
   Confirm they pass. Commit.
5. Write failing loop tests in `candle/run.rs`, each a short hand-computed series:
   - a bar at force-close time closes at open with `EndOfDay` and nothing is
     queued;
   - a queued long exit closes an open short with reason `Signal`;
   - an exit and an entry queued on the same bar fill exit first;
   - cap 2 with 1.5 open trims an entry of 1 to 0.5 and skips at 2.0 open;
   - a winning trade raises the next safety-margin size;
   - a non-tradable bar queues nothing even when `entry` is set;
   - `force_close_at_end` closes at the last close with `ForceClose`, and without
     it the trade stays open with NaN exit fields;
   - the last bar of a day closes at close with `EndOfDay`;
   - dates for a time before 1970 split correctly;
   - `InvalidSignal` for `entry = 2`, and `LengthMismatch` naming `strength`.

   Confirm they fail. Implement `run_candle`, `TradeLedger` and `DecisionTrace`.
   Confirm they pass. Commit.
6. Add exporter unit tests for `scripted_strategy.py`, `candle_engine.py` and
   `decision_step.py`: the scripted strategy returns long wins over short when
   both are scripted; the ledger encoder fails on an ambiguous entry time; the
   decision encoder writes reason code 11 for `exit_reason=None`. Confirm they
   fail. Write the three modules with the scenarios in the decisions and register
   both families. Confirm they pass. Commit.
7. In the full backend environment run `make fixtures-backend`. Confirm only
   `candle_engine/` and `decision_step/` files appeared, that every engine
   scenario opens at least one trade and hits the edge it names (check each by
   reading its ledger), and that the decision-step export's evaluator assertion
   passed for all seven. Commit the 24 files.
8. Write `tests/candle_engine_gate.rs`: accounting against the 17 ids and
   `BACKEND_REV`; rebuild inputs and `CandleConfig` per case; run `run_candle`;
   compare every ledger column under the exact policy. Run it and confirm it fails
   first on encoding, then fix encoding only. A value difference is a kernel
   defect: add a failing unit test for it in step 3, 4 or 5's module, fix, and
   rerun. Confirm it passes. Commit.
9. Add negative controls to the engine gate (ULP price, exit bar +1, reason
   `Signal` to `EndOfDay`, checksum, unaccounted file), the double-run check over
   `CandleRun`, and a prefix check that the decision trace over the first `k` bars
   of each scenario equals the full trace's first `k` bars for `k` in
   {1, n/3, 2n/3, n}. Confirm they pass. Commit.
10. Write `tests/decision_step_gate.rs`: for each decision-step scenario create one
    `DecisionStep`, call `decide` for every bar with the scenario's open trade
    (entry bar resolved as the exporter did), compare exits and entry per bar
    exactly, and compare `Sizing::size` at close with initial capital against the
    recorded `requested_quantity`. Confirm it fails before wiring, then passes.
    Commit.
11. Write failing wheel tests in `tests/test_engine.py`: `run_candle` on the
    `k07` inputs returns ledger arrays equal to the fixture; `exit_reason_text`
    for a rule exit is the rule id; `DecisionStep` with trade id
    `"golden-parity-trade"` reproduces `decision_step/d03` bar by bar;
    `required_columns({"stop_loss_atr": 2, "atr_period": "21", "donchian_exit_period": 10.0})`
    is `["atr_21", "donchian_high_10", "donchian_low_10"]`; `enabled_rules` for
    `{"trailing_stop_pct": 0.03, "max_bars_in_trade": 5}` is
    `["trailing", "time_stop"]`; `max_position` for an inverse-volatility mapping
    with `max_contracts` 5 is 5.0 and without it is `None`; after three `decide`
    calls with a trailing stop, `state("golden-parity-trade")["trailing"]["extreme"]`
    equals the fixture's recorded extreme and `state("unknown")` is `None`; a float16 `close`
    and a 399-long `strength` each raise `ValueError` naming the argument; an
    `entry` of dtype int64 raises `TypeError` naming `entry`. Confirm they fail.
    Implement `q-py/src/engine.rs` and register `q_core.engine`. Confirm they pass
    under `make wheel-test`. Commit.
12. Add `tests/bench_candle_kernel.py` and the `bench-candle-kernel` target: clone
    `q_backend` at `BACKEND_REV` into a temporary directory, `uv sync --frozen`,
    install the wheel into that environment, build a 50,000-bar
    `synthetic_ohlcv` series with scripted decisions and a trailing stop, and time
    `BacktestEngine._run_single_chunk` and `q_core.engine.run_candle` five times
    each, asserting equal ledgers before printing individual and median times and
    bars per second. Commit.
13. Update README fixture-protocol and benchmark lines. Confirm the Q-022,
    Q-025 and Q-026 gates pass unchanged and `make parity-isolation` passes.
    Release preparation: bump `[workspace.package] version` and `pyproject.toml`
    `version` to today's date per `RELEASING.md` step 2, so the human can tag the
    merge that Q-028 pins, and update the Qt harness's expected version as the
    Q-022 release did. Commit.
14. Human step, matching human-verifiable criterion 1: in the full backend
    environment run `time make fixtures-backend-check`.
15. Human step, matching human-verifiable criterion 2: run
    `make bench-candle-kernel` and record the figures.
16. Run the full validation suite. Commit.

## Validation

- **Unit:** side cost bits and the no-config zero; each sizer's floors, strength
  scaling, min and max rules, missing volatility, validation order and maximum
  position; decision order, rule suppression of strategy exits, holding-period
  boundary and off-grid entries, entry passthrough; loop order A to E, both-sides
  close with the first reason, cap trim and skip, compounding, tradable mask,
  force-close at end, pre-1970 dates, signal and length validation.
- **Integration:** 17 candle-engine fixtures (every ledger column) and 7
  queued-signal parity fixtures (every queued exit and entry, and requested
  quantity) through the Q-021 gate under the exact policy; wheel round trip for a
  run and for the stateful step with string ids.
- **Regression:** Q-022, Q-025 and Q-026 gates unchanged; `make parity-isolation`;
  existing wheel tests unchanged.
- **Negative controls:** ULP price, exit bar shift, reason change, checksum,
  unaccounted file.
- **Determinism and causality:** double run over trades and trace; prefix check
  of the decision trace at four lengths.
- **Measurement:** kernel against the pinned Python engine on 50,000 bars, five
  runs each, individual and median wall time and bars per second.
- **Manual:** fixture regeneration in the full backend environment with time.

```bash
cd /home/gui/projects/q/q_core
make check
cargo test -p q-engine --lib candle
cargo test -p q-engine --test candle_engine_gate --test decision_step_gate -- --nocapture
cargo test -p q-engine --test exit_rules_gate
uv run --frozen --project tools/reference python -m unittest discover -s tools/reference
make wheel-test
make parity-isolation

# human (steps 14 and 15)
time make fixtures-backend-check
make bench-candle-kernel
```

## Handoff

Report the 17 engine scenario ids with each one's trade count and the edge its
ledger shows being hit, and the 7 decision-step scenarios with their bar counts
and the number of bars that queued an exit. Confirm that the exporter's
evaluator-versus-reference assertion passed for all seven. Report every kernel
value that first differed from a fixture, its cause, and the unit test that now
pins it, and confirm no kernel code was changed to fit an encoding problem. List
every `suboptimal_flops` allowance. Report negative controls, determinism and
prefix-trace results, and confirm `BACKEND_REV` is unchanged. Report any
difference found between Q-024's merged signal columns and this plan. From the
human steps, report the regeneration diff and time, and the benchmark's
individual and median wall times and bars per second for the Python engine and
the kernel.
