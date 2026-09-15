# Q-026 implementation plan: Exit-rule state machines

**Status:** authoritative in the [Q project board](https://github.com/users/GuilhermeFortuna/projects/2)  
**Specification:** [`../specs/Q-026-exit-rule-state-machines-spec.md`](../specs/Q-026-exit-rule-state-machines-spec.md)  
**Depends on:** Q-025

## Current-system context

`crates/q-engine/src/lib.rs` (18 lines) is a skeleton: `#![forbid(unsafe_code)]`
and a `CRATE_NAME` constant. Its manifest already depends on `q-indicators` and
`q-buffers` and has no dev-dependencies. `q-buffers` (Q-025) provides `BarFrame`
with `open()`/`high()`/`low()`/`close()` slices and `column(name) ->
Option<&Column>`, where `Column::Float64(Vec<f64>)` carries indicator values with
NaN for missing. `q-indicators/src/ieee.rs` is the audited home of the one
`float_cmp` exception in that crate, and the workspace lint table denies
`float_cmp`, `float_cmp_const`, `imprecise_flops` and `suboptimal_flops`.
`q-parity` offers `load_family`, `check_accounting`, `compare_f64` under
`Policy::Exact`, `check_double_run_with` over any `BitEq` trace, and
`check_prefix_causal` over a `KernelFn` that maps `KernelInputs` (named f64
columns) to `KernelOutputs` (named f64 columns). The `bar_window` family
(`tools/reference/families/bar_window.py`) is the template for a backend family:
`environment = "backend"`, `policy = {"kind": "exact"}`, provenance with git
blob ids of the backend sources it drives, inputs from `test_goldens.synthetic_ohlcv`,
listed in `BACKEND_FAMILIES` in the `Makefile`, and read by
`crates/q-buffers/tests/bar_window_gate.rs`. `BACKEND_REV` is `067e29c`, which is
`q_backend` `development` before Q-023.

In `q_backend` at that pin, `ExitStrategy` (`backtesting/exit_strategy.py`)
builds `self._rules = enabled_rules(params)` (`exit_rules/registry.py:59`, a
filter over `EXIT_RULES`, line 24: `LEGACY_EXIT_RULES` = fixed_sl, atr_sl,
fixed_tp, atr_tp, trailing, then `SPECIALIZED_EXIT_RULES` = chandelier,
breakeven, psar, profit_target_ratchet, time_stop, donchian_stop).
`check_exits` (lines 74-90) prunes `_state` to the ids of `open_trades`
(line 59), returns `[]` when none are open, and for each trade and each rule
calls `rule.on_bar(trade, row, state, params)`, then skips `should_exit` unless
`_columns_ready` (line 67: every `required_columns` value is present and not
`pd.isna`), then on the first `True` appends
`Signal(CLOSE, exit_reason=rule.id)` and `break`s. `_bar_prices`
(`legacy.py:16`) reads `close` with default 0.0 and `high`/`low` defaulting to
that close; `_is_long` compares `trade.action` to `"BUY"`. Stateful rules use
`state` dicts: trailing `extreme` (initialised `max(entry, high)`), chandelier
`peak` (entry, then `max(peak, high)`), breakeven `armed`, psar
`sar/ep/af/prior_low/prior_prior_low` or the short-side highs
(`parabolic_sar.py:39-100`, first bar initialises and returns), ratchet
`armed/ratchet` (`on_bar` returns early when ATR is missing), time_stop `bars`.
Parameters are coerced at each use with `float(...)`/`int(...)` and defaults
(`atr_period` 14, `psar_af_step` 0.02, `psar_af_max` 0.2). `required_columns`
(line 75) is `sorted(set(...))`. Tests live in `tests/backtesting/test_exit_rules.py`
(46 tests) and `test_exit_strategy.py`. The gap is that none of this exists in
`q_core`, and no fixture pins the per-bar state the Python rules carry.

## Interfaces produced

```rust
// crates/q-engine/src/lib.rs   (changed)
pub mod exits;
pub use exits::{ExitBook, ExitDecision, ExitError, ExitInputs, ExitParams, ExitRuleId,
                ExitRuleSet, OpenPosition, ParamValue, PositionKey, RuleState, Side};

// crates/q-engine/src/exits/pyops.rs   (new; the audited float module for this crate)
pub(crate) fn py_max(a: f64, b: f64) -> f64;   // Python max(a, b): b if b > a else a
pub(crate) fn py_min(a: f64, b: f64) -> f64;   // Python min(a, b): b if b < a else a
pub(crate) fn is_missing(x: f64) -> bool;       // pd.isna on a float: NaN only (inf is present)
```

```rust
// crates/q-engine/src/exits/params.rs   (new)
/// A numeric parameter as the caller holds it; coerced with Python's float()/int() rules.
#[derive(Clone, Copy, Debug)]
pub enum ParamValue { Bool(bool), Int(i64), Float(f64) }
impl ParamValue {
    pub fn to_float(self) -> f64;                                   // Bool 0.0/1.0, Int as f64
    pub fn to_int(self, name: &'static str) -> Result<i64, ExitError>; // Float truncates toward zero; NaN/inf rejected like int()
}

/// Every exit parameter, defaults equal to the backend's `params.get(name, default)`.
#[derive(Clone, Debug)]
pub struct ExitParams {
    pub stop_loss_pct: f64, pub stop_loss_atr: f64,
    pub take_profit_pct: f64, pub take_profit_atr: f64,
    pub trailing_stop_pct: f64,
    pub atr_period: i64,                   // 14
    pub chandelier_atr_mult: f64,
    pub breakeven_trigger_pct: f64, pub breakeven_offset_pct: f64,
    pub psar_af_start: f64, pub psar_af_step: f64, pub psar_af_max: f64,   // 0.0, 0.02, 0.2
    pub target_ratchet_atr: f64,
    pub max_bars_in_trade: i64,
    pub donchian_exit_period: i64,
}
impl Default for ExitParams { fn default() -> Self; }
impl ExitParams {
    /// Unknown names are ignored, as the backend ignores non-exit parameters.
    pub fn from_pairs<'a>(pairs: impl IntoIterator<Item = (&'a str, ParamValue)>) -> Result<Self, ExitError>;
}

#[derive(Clone, Debug, PartialEq)]
pub enum ExitError {
    InvalidParameter { name: &'static str, reason: &'static str },
    LengthMismatch { column: String, expected: usize, actual: usize },
}
impl core::fmt::Display for ExitError { /* "<name>: <reason>" */ }
impl std::error::Error for ExitError {}
```

```rust
// crates/q-engine/src/exits/rules.rs   (new)
/// Rule identities in backend registry order; the discriminant is the fixture exit code.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ExitRuleId {
    FixedStopLoss = 0, AtrStopLoss = 1, FixedTakeProfit = 2, AtrTakeProfit = 3, Trailing = 4,
    Chandelier = 5, Breakeven = 6, ParabolicSar = 7, ProfitTargetRatchet = 8, TimeStop = 9,
    DonchianStop = 10,
}
impl ExitRuleId {
    pub const REGISTRY_ORDER: [ExitRuleId; 11];
    pub fn as_str(self) -> &'static str;   // "fixed_sl", "atr_sl", ..., "donchian_stop"
    pub fn is_enabled(self, params: &ExitParams) -> bool;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side { Long, Short }

/// Per-rule state for one position; `None` mirrors a key absent from the Python state dict.
#[derive(Clone, Debug, Default)]
pub struct RuleState {
    pub trailing_extreme: Option<f64>,
    pub chandelier_peak: Option<f64>,
    pub breakeven_armed: bool,
    pub psar: Option<PsarState>,
    pub ratchet: Option<f64>,               // Some iff armed
    pub time_stop_bars: i64,                // 0 until the first update
}
#[derive(Clone, Copy, Debug)]
pub struct PsarState { pub sar: f64, pub ep: f64, pub af: f64,
                       pub prior: f64, pub prior_prior: Option<f64> }  // lows for long, highs for short
```

```rust
// crates/q-engine/src/exits/book.rs   (new)
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PositionKey(pub u64);            // caller-assigned identity (trade ordinal, interned id)

#[derive(Clone, Copy, Debug)]
pub struct OpenPosition { pub key: PositionKey, pub side: Side, pub entry_price: f64 }

pub struct ExitRuleSet { /* params: ExitParams, enabled: Vec<ExitRuleId> in registry order */ }
impl ExitRuleSet {
    pub fn new(params: ExitParams) -> Self;
    pub fn params(&self) -> &ExitParams;
    pub fn enabled(&self) -> &[ExitRuleId];
    pub fn required_columns(&self) -> Vec<String>;   // sorted, de-duplicated
}

/// Column views for one evaluation sequence; every slice has the frame's length.
pub struct ExitInputs<'a> {
    pub close: &'a [f64],
    pub high: Option<&'a [f64]>,             // None reads close, as _bar_prices does
    pub low: Option<&'a [f64]>,
    pub atr: Option<&'a [f64]>,              // atr_<atr_period>; None = column absent
    pub donchian_high: Option<&'a [f64]>,    // donchian_high_<donchian_exit_period>
    pub donchian_low: Option<&'a [f64]>,
}
impl<'a> ExitInputs<'a> {
    pub fn from_frame(frame: &'a q_buffers::BarFrame, rules: &ExitRuleSet) -> Result<Self, ExitError>;
    pub fn from_slices(close: &'a [f64], high: Option<&'a [f64]>, low: Option<&'a [f64]>,
                       named: &dyn Fn(&str) -> Option<&'a [f64]>, rules: &ExitRuleSet) -> Result<Self, ExitError>;
    pub fn len(&self) -> usize;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExitDecision { pub key: PositionKey, pub rule: ExitRuleId }

/// Exit-rule state for every open position of one strategy.
pub struct ExitBook { /* rules: ExitRuleSet, states: BTreeMap<PositionKey, RuleState> */ }
impl ExitBook {
    pub fn new(rules: ExitRuleSet) -> Self;
    pub fn rules(&self) -> &ExitRuleSet;
    /// ExitStrategy.check_exits for bar `bar`: prune, then update/decide per position and rule.
    pub fn evaluate(&mut self, positions: &[OpenPosition], inputs: &ExitInputs<'_>, bar: usize)
        -> Vec<ExitDecision>;
    pub fn state(&self, key: PositionKey) -> Option<&RuleState>;
}
```

```
crates/q-engine/Cargo.toml                    dev-dependencies: q-parity, serde_json
crates/q-engine/tests/exit_rules_gate.rs      new: fixture gate, negative controls, determinism, prefix causality
tools/reference/families/exit_rules.py        new: backend family driving ExitStrategy.check_exits
tools/reference/export_reference.py           FAMILIES gains "exit_rules"
tools/reference/test_export_reference.py      exporter unit tests for the family
fixtures/reference/exit_rules/                new: 16 fixture files
Makefile                                      BACKEND_FAMILIES := bar_window exit_rules
README.md                                     fixture protocol lines for exit_rules
```

## Implementation decisions

- **Rules are an enum dispatched with `match`, not trait objects.** The set is
  closed (eleven registered rules), the fixture exit code needs a stable
  integer per rule, and a `match` keeps the registry order visible in one
  `REGISTRY_ORDER` array. A trait object per rule would allocate per rule set
  and hide the order in construction code.

- **State is one `RuleState` struct per position with a field per stateful
  rule, and `Option` wherever Python tests `"key" not in state`.** The Python
  rules branch on key presence (trailing and psar initialise on first sight,
  `state.get("sar")` is `None` before the first update). A zero default would
  make "not yet initialised" indistinguishable from a real 0.0 level. One flat
  struct keeps state `Clone` for the double-run check and cheap to snapshot for
  fixtures.

- **Position identity is a caller-assigned `u64`.** Python keys state by
  `trade.id`, a UUID in the engine and `exec-<deployment>` in the evaluator. The
  kernel in Q-027 has trade ordinals, and the Python projection in Q-027 interns
  string ids into keys. Strings in the core would allocate on every lookup of a
  per-bar hot path for no semantic gain.

- **`ExitBook::evaluate` prunes first, then returns early on an empty slice.**
  That is `check_exits` lines 75-77 in order. Pruning before the early return is
  what makes a reused identity start fresh after a bar with no open trades, and
  the `c03` fixture pins it.

- **Update, readiness check, decision, break: in that order and nothing else.**
  `on_bar` runs before `_columns_ready`, so a rule's state advances on bars
  where its decision is skipped, and the `break` skips later rules' updates. Both
  change later exits, so both are reproduced and both have fixture coverage
  (`c01` enables all eleven rules; ATR warm-up bars cover readiness).

- **Readiness is computed from the columns each rule declares, not from whatever
  a rule reads.** Psar, trailing, breakeven and time stop declare none and are
  always ready. ATR rules declare `atr_<period>`; the ratchet's `on_bar` has its
  own early return on missing ATR, which is reproduced inside the rule rather
  than folded into readiness, because folding it would also skip the update on a
  ready bar.

- **Python operand semantics live in `pyops.rs`, the crate's single audited
  float module.** `max(a, b)` in Python returns `a` unless `b > a`; Rust's
  `f64::max` returns the non-NaN operand. They differ whenever a high or low is
  NaN, and trailing, chandelier, psar and ratchet all call max or min. Comparisons
  (`<=`, `>=`) already agree with Python for NaN (false). `is_missing` is NaN only,
  because `pd.isna(inf)` is false.

- **Arithmetic is written operation for operation, and `suboptimal_flops` is
  allowed with a reason only in the rule arithmetic.** Clippy suggests
  `mul_add` for `entry - (mult * atr)` and `sar + af * (ep - sar)`. A fused
  multiply-add rounds once instead of twice and changes bits, and the exact
  policy would reject it. Each rule's level expression keeps Python's
  parenthesisation (`entry * (1.0 - pct)`, `(high - entry) / entry`), and the
  `allow` sits on the functions that compute levels, never crate-wide.

- **Parameters are typed in the core and coerced by `ParamValue`.** The backend
  passes a dict of mixed ints, floats and bools and coerces at each use.
  `to_int` truncates toward zero and rejects NaN and infinity, which are exactly
  the values `int()` raises on. String parameters are not representable here;
  Q-027's projection calls Python's own `float()`/`int()` on such values before
  building `ParamValue`, so no string parsing is reimplemented in Rust.

- **Parameters Python would accept are not validated further.** A negative
  `psar_af_step` or an `atr_period` of 0 is accepted today and produces
  predictable (if odd) output, so rejecting it would change what a saved strategy
  does. The only construction errors are the coercion failures above.

- **`ExitInputs` resolves the named columns once per sequence.** Looking up
  `atr_<period>` by string on every bar is the pandas `.get` cost the kernel is
  meant to remove. An absent column becomes `None` and is missing on every bar,
  matching `data.get(col, None)`. `from_frame` never sees an absent high or low,
  because a Q-025 frame always has them; `from_slices` keeps the fallback for the
  close-only series the engine still accepts.

- **The fixture family drives `ExitStrategy.check_exits` directly with a
  scripted schedule of open positions, not through the engine.** Positions stay
  open for their scheduled interval regardless of exits, which is how the forward
  evaluator uses the rules and which exercises state after a rule has fired.
  Driving the engine would close positions on the next bar and hide every state
  transition after the first exit. The engine loop is Q-027's family.

- **Fixture inputs carry the ATR and Donchian columns as exported values.**
  Rust never recomputes them. Recomputing would make this gate depend on Q-022's
  kernels and turn an ATR difference into an exit-rule failure.

- **Fixture layout: one file per rule, plus five composition files.**
  - `r01_fixed_sl` through `r11_donchian_stop`: over
    `synthetic_ohlcv(n=160)` with `atr_14` and `donchian_high_20`/`donchian_low_20`
    computed by `augment_indicator_frame` at the pin, one long and one short
    position opened at bar 5 (inside ATR warm-up) and held to bar 159, with
    parameters chosen so each rule fires at least once per side.
  - `c01_all_rules`: all eleven enabled on the same schedule.
  - `c02_close_only`: high and low absent, every rule enabled.
  - `c03_multi_positions_reuse`: three positions with overlapping intervals, one
    identity closed at bar 60 and reopened at bar 70 with a different side and
    entry price, and a bar with no open positions between.
  - `c04_psar_cap`: `psar_af_step` 0.1, `psar_af_max` 0.25 on a trending
    segment, so the cap and both clamps are reached.
  - `c05_time_stop_one`: `max_bars_in_trade` 1.

  Each case stores the inputs, the parameters, the schedule
  (key, side, entry price, first bar, last bar), and per position per bar: the
  exit code (`-1` for none, else the `ExitRuleId` discriminant) and every
  `RuleState` field (float64 with NaN for `None`; flags and counts as int64 with
  `-1` for absent). The exporter reads `ExitStrategy._state` for the state, as
  `bar_window` reads `evaluator._rolling`.

- **The gate test exposes each scenario as a Q-021 `KernelFn` over its input
  columns whose outputs are the per-position trace columns.** That reuses
  `check_prefix_causal` and `compare_f64` unchanged instead of writing a second
  causality checker. Integer codes are exact in f64.

## Ordered implementation

1. Work on the branch `Q-026-exit-rule-state-machines` in `q_core`, created from
   `development` by `./work start`. Confirm `BACKEND_REV` is still `067e29c`
   and that `git -C <backend checkout> diff 067e29c development -- src/q_backend/backtesting/exit_rules src/q_backend/backtesting/exit_strategy.py`
   is empty. If it is not, stop and report, because the fixtures would no
   longer describe the backend that Q-028 swaps.
2. Write failing unit tests in `exits/pyops.rs`: `py_max(NaN, 1.0)` is NaN,
   `py_max(1.0, NaN)` is 1.0, `py_min(NaN, 1.0)` is NaN, `py_max(-0.0, 0.0)`
   returns `-0.0` (no `>`), `is_missing(f64::INFINITY)` is false. Confirm they
   fail. Implement. Confirm they pass. Commit.
3. Write failing tests in `exits/params.rs`: defaults equal `atr_period` 14,
   `psar_af_step` 0.02, `psar_af_max` 0.2 and zero elsewhere; `Float(14.7)` for
   `atr_period` gives 14 and `Float(-3.9)` gives -3; `Float(NaN)` for
   `max_bars_in_trade` is `InvalidParameter` naming it; `Bool(true)` for
   `stop_loss_pct` is 1.0; an unknown name is ignored. Confirm they fail.
   Implement. Confirm they pass. Commit.
4. Write failing tests in `exits/rules.rs` against expectations copied from the
   backend registry at the pin: `REGISTRY_ORDER` ids equal the backend's
   `[r.id for r in EXIT_RULES]`; with only `trailing_stop_pct=0.03` enabled is
   `[Trailing]`; with all eleven enabling parameters set, enabled order is the
   full registry order; `required_columns` for
   `{stop_loss_atr: 2, chandelier_atr_mult: 3, atr_period: 21, donchian_exit_period: 10}`
   is `["atr_21", "donchian_high_10", "donchian_low_10"]`. Confirm they fail.
   Implement `ExitRuleId`, `is_enabled` and `ExitRuleSet`. Confirm they pass.
   Commit.
5. Write failing rule-level unit tests, one module per rule family, each a
   hand-computed bar sequence on a long and a short position:
   - fixed and ATR stop and target at the exact boundary (`low == level` exits);
   - trailing initialises to `max(entry, high)` and then trails;
   - chandelier with ATR NaN on bar 0 updates the peak but does not exit;
   - breakeven arms on bar 2 and exits on bar 3 at `entry * (1 + offset)`;
   - psar first bar initialises only; bar 3 clamps to the prior-prior low; AF
     stops at `af_max`;
   - ratchet does not arm on a NaN-ATR bar even when high passes the level;
   - time stop with `max_bars_in_trade = 3` exits on the third evaluation;
   - Donchian with both columns absent never exits.

   Confirm they fail. Implement `RuleState` and each rule's update and decision
   with the `suboptimal_flops` allow on the level functions only. Confirm they
   pass. Commit.
6. Write failing tests for `ExitBook::evaluate`: two positions both hit by a
   stop yield two decisions in slice order; with `fixed_sl` and `time_stop`
   enabled and the stop firing, the time-stop bar count of that position does not
   advance on that bar; a key absent from the next call's slice loses its state,
   and a call with an empty slice clears all state; a reused key starts with
   `trailing_extreme == None`; `ExitInputs::from_slices` with `high = None`
   reads the close. Confirm they fail. Implement `ExitBook` and `ExitInputs`.
   Confirm they pass. Commit.
7. Add exporter tests to `tools/reference/test_export_reference.py` for the
   `exit_rules` family: the schedule encoder rejects a first bar after the last
   bar; the state encoder maps an absent key to NaN or `-1`; a fixture round
   trip preserves the checksum. Confirm they fail. Write
   `families/exit_rules.py` with the sixteen scenarios in the decisions,
   register it in `FAMILIES`, add it to `BACKEND_FAMILIES`. Confirm the unit tests
   pass. Commit.
8. In the full backend environment, run `make fixtures-backend` and commit the
   sixteen files under `fixtures/reference/exit_rules/`. Confirm with
   `git status` that no `bar_window` or `indicators` file changed. Confirm every
   `r*` fixture has at least one exit per side, and drop or re-parameterise any
   that does not before committing. Commit.
9. Add `q-parity` and `serde_json` as dev-dependencies of `q-engine`. Write
   `tests/exit_rules_gate.rs`: load the family, check accounting against the
   sixteen ids and `BACKEND_REV`, rebuild each scenario's `ExitParams`, schedule
   and `ExitInputs`, run the book bar by bar, and compare exits and states under
   the exact policy. Run it and confirm it fails until the trace encoding matches
   the exporter. Fix only encoding, never rule code, and if a rule value differs,
   treat it as a rule defect and add a unit test for it in step 5's module first.
   Confirm it passes. Commit.
10. Add negative controls to the gate test, each asserting failure: one state
    value moved by one ULP, one exit moved one bar later, one exit code changed
    from `Trailing` to `FixedStopLoss`, a value edited without updating its
    checksum, and a stray `exit_rules/zz_extra.json` in a temporary copy of the
    fixture root. Add the double-run check (`check_double_run_with` over the
    trace) and the prefix check (`check_prefix_causal` at prefix lengths 1, 17,
    80 and the full length). Confirm they pass. Commit.
11. Document the crate: module docs for `exits` listing each rule, its
    parameters and defaults, and the reproduced quirks (skipped later-rule
    updates, time stop counting the fill bar, ratchet not arming on missing ATR,
    Python NaN operand rules). Add the README fixture-protocol lines for
    `exit_rules`. Confirm `make parity-isolation` passes and that
    `cargo tree -p q-engine -e normal` lists only `q-indicators` and `q-buffers`
    from the workspace. Commit.
12. Human step, matching human-verifiable criterion 1: in the full backend
    environment run `time make fixtures-backend-check` and report the diff (none
    expected) and the time.
13. Human step, matching human-verifiable criterion 2: review the `q-engine`
    docs against `q_backend/src/q_backend/backtesting/exit_rules/` at the pin.
14. Run the full validation suite. Commit.

## Validation

- **Unit:** Python max/min NaN operand rules; parameter coercion and defaults;
  enablement, order and required columns against the backend registry; each
  rule's boundary, initialisation and missing-value behaviour on long and short;
  book pruning, identity reuse, first-rule-wins and skipped later updates; close
  fallback for absent high and low.
- **Integration:** sixteen backend-exported fixtures through the Q-021 gate under
  the exact policy, exits and every state field per position per bar.
- **Regression:** existing `indicators` and `bar_window` families unchanged;
  `make parity-isolation`; dependency graph of `q-engine` unchanged.
- **Negative controls:** ULP state change, one-bar exit shift, wrong reason,
  checksum mismatch, unaccounted file.
- **Determinism and causality:** double run per scenario; prefix causality at
  four prefix lengths per scenario.
- **Manual:** fixture regeneration in the full backend environment with time
  reported; crate documentation reviewed against the backend registry.

```bash
cd /home/gui/projects/q/q_core
make check
cargo test -p q-engine --lib exits
cargo test -p q-engine --test exit_rules_gate -- --nocapture
uv run --frozen --project tools/reference python -m unittest discover -s tools/reference
make parity-isolation
cargo tree -p q-engine -e normal

# human (steps 12 and 13)
time make fixtures-backend-check
cargo doc -p q-engine --no-deps --open
```

## Handoff

Report the sixteen fixture ids with the number of exits per side in each, and
confirm that no `r*` fixture was left without an exit on either side. Report any
rule whose Rust result first differed from the fixture, what the cause was, and
the unit test that now pins it. Confirm that no rule code was changed to match
an encoding problem. List every place `suboptimal_flops` is allowed. Report the
negative-control results, the determinism and prefix-causality results, and
confirm `BACKEND_REV` is unchanged and the backend's exit-rule sources are
unchanged between the pin and `development`. From the human steps, report the
regeneration diff and time, and the review outcome with any rule, parameter or
default found missing.
