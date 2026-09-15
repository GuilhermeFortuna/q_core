# Q-026: Exit-rule state machines

**Status:** authoritative in the [Q project board](https://github.com/users/GuilhermeFortuna/projects/2)  
**Project direction:** [`q_contracts/docs/system-architecture.md` §3.3, §5, §9 invariant 1, §10 Phase 2](https://github.com/GuilhermeFortuna/q_contracts/blob/f2273a88e52c5b9a8ad5ac9d7eb27f8069cf643d/docs/system-architecture.md#33-rust-scope-in-execution)  
**Depends on:** Q-025  
**Implementation plan:** [`../plans/Q-026-exit-rule-state-machines-plan.md`](../plans/Q-026-exit-rule-state-machines-plan.md)

## Purpose

Every stop, target, trailing stop and time stop that closes a trade in Q is a
Python object in `q_backend` that is asked, once per bar and once per open
trade, to update its state and decide whether to exit. The backtest engine and
the forward evaluator both call it that way. Eleven rules keep per-trade state
across bars (the in-trade extreme, the parabolic SAR recursion, the break-even
and ratchet arming flags, the bar counter), so they are exactly the kind of
semantics architecture §3.3 assigns to Rust, and the candle kernel (Q-027)
cannot run a bar loop while they live in Python. `q-engine` is still an empty
crate. This task gives it the exit rules as deterministic state machines over
columnar bar inputs, and proves them equal to today's Python rule by rule and
bar by bar, state included, before any loop depends on them.

## Requirements

### Rule catalog

- `q-engine` implements the eleven exit rules `q_backend` registers today, under
  their existing rule identifiers: fixed stop loss, ATR stop loss, fixed take
  profit, ATR take profit, percent trailing stop, chandelier exit, break-even
  stop, parabolic SAR stop, profit-target ratchet, time stop, and Donchian
  channel stop.
- A rule set is built from the same parameter names and defaults the backend
  uses. A rule is enabled exactly when the backend enables it: its enabling
  parameter is greater than zero. Parameters that are not exit parameters are
  ignored, as they are today.
- Enabled rules are evaluated in the backend's registry order: the five legacy
  rules first, then chandelier, break-even, parabolic SAR, ratchet, time stop,
  and Donchian stop. Order is part of the semantics, because the first rule
  that fires decides the exit reason.
- A rule set reports the indicator columns its enabled rules read, as the same
  sorted, de-duplicated list of names the backend reports (`atr_<period>`,
  `donchian_high_<period>`, `donchian_low_<period>`). Parameter coercion that
  decides those names (an ATR period given as a float, for example) matches the
  backend's.

### Evaluation on a bar

- A bar is evaluated for an ordered set of open positions. Each position has an
  identity chosen by the caller, a side, and an entry price.
- Before evaluating, the state of every position that is no longer open is
  discarded. A position that reappears later under the same identity starts
  from fresh state, exactly as the backend discards state keyed by trade id.
- For each position, in order, each enabled rule in order first updates its
  state from the bar and then, if every indicator column it reads has a
  non-missing value on that bar, decides whether to exit. The first rule that
  decides to exit produces one exit for that position, labelled with the rule's
  identifier, and the remaining rules are neither updated nor asked for that
  position on that bar. That skipped update is today's behaviour and is kept.
- A rule whose indicator value is missing on a bar still updates any state that
  does not depend on that value, and a rule whose update needs the missing value
  leaves its state unchanged, exactly as each Python rule does.
- Bar prices follow the backend's fallbacks: a missing high or low reads the
  close. Comparisons, minima and maxima keep Python's behaviour when a value is
  NaN, including which operand wins when one side is NaN.
- Arithmetic reproduces the backend's results bit for bit: every stop, target,
  trail, SAR and ratchet level is computed with the same operations in the same
  order, and no operation is fused or reordered.
- Exits are reported in position order. A bar with no open positions reports no
  exits and leaves no state behind.

### Inputs

- Rules read bar prices and indicator values as columns: from a Q-025 bar frame
  by column name, or from equally long slices supplied by the caller. No rule
  reads a row object, and no value is copied per bar.
- A required indicator column that is absent is treated as missing on every
  bar, as a missing key is today. It is not an error, because the backend relies
  on that during indicator warm-up.
- Invalid rule parameters are rejected when the rule set is built, with an error
  that names the parameter. Where today's Python accepts a value and behaves
  predictably, the value is accepted and the behaviour is reproduced rather than
  newly rejected.

### State visibility

- Each position's per-rule state can be read after any bar, in terms a fixture
  can compare: the trailing extreme, the chandelier peak, the break-even and
  ratchet arming flags and ratchet level, the SAR, extreme point, acceleration
  factor and prior highs or lows, and the time-stop bar count.
- State depends only on the bars evaluated so far for that position. Evaluating
  a prefix of a bar sequence yields the same exits and states on that prefix as
  evaluating the whole sequence.

### Parity and determinism

- Parity with `q_backend` is proven by reference fixtures exported from its
  exit strategy through the Q-021 exporter and checked by the Q-021 gate
  library, in the Q-021 fixture format, under the exact policy. Exits compare as
  bar and reason, and every state value compares bit for bit.
- The fixtures are exported at the existing reference pin. No backend change
  between that pin and this task touches exit rules, and moving the pin would
  regenerate every family. The reference stays the Python rules and is never
  `q_core` compared with itself.
- Reproducing the fixtures needs the full `q_backend` environment, like the
  Q-025 window fixtures, and follows the same documented decision about CI.
- Scenarios cover every rule on long and short positions; state initialisation
  on the first bar in a trade; warm-up bars where indicator values are missing;
  frames without high and low; precedence when several rules fire on one bar
  (including a later rule whose state update is skipped); several positions at
  once; a position closed and its identity reused; parabolic SAR clamping and
  its acceleration cap; ratchet arming then trailing; break-even arming then
  exiting; and a time stop of one bar.
- The same evaluation sequence run twice in one process produces bit-identical
  exits and states, checked by the Q-021 double-run determinism check.

### Preserved guarantees

- `q-engine` stays free of unsafe code, input and output, clocks and
  environment access. Its dependency direction (`q-indicators`, `q-buffers`) is
  unchanged, and no binding crate gains a dependency on `q-parity`.
- Every existing public item, fixture family and gate in the workspace keeps
  passing unchanged. `q_core.version()` and `q_core.contracts_rev()` are
  unchanged.

## Constraints and non-goals

- **No bar loop, fills, sizing, costs, or holding period.** Deciding that a
  position should exit is this task. Queueing that exit, filling it at the next
  open, and everything else in the engine's bar loop is Q-027. The strategy's
  fixed holding period is declared by the strategy in Q-024 and consumed by the
  kernel in Q-027; it is not an exit rule.
- **No Python projection.** The candle kernel's decision step in Q-027 is the
  only consumer and projects exit evaluation with it. Projecting the rule set on
  its own would create a second per-bar entry point that the evaluator could
  call around the step.
- **No change to `q_backend`.** Backtests move to these rules in Q-028 and the
  forward evaluator in Q-031. Until then the Python rules stay the reference.
- **No rule metadata in Rust.** Labels, descriptions, exit groups, parameter
  specs, search ranges and presets describe rules to the UI and the optimizer.
  They are not semantics and stay in the backend.
- **No fixes to quirks found along the way.** A firing rule skips the state
  update of every later rule on that bar; the time stop counts the bar on which
  the trade filled; the ratchet cannot arm on a bar where ATR is missing. These are reproduced,
  because the goldens exist to hold results still.
- **No new rules and no tick stops.** The tick kernel's price-point stop loss
  and take profit are a different semantic and belong to Q-029.
- **No move of the `q_backend` reference pin.**

## Acceptance criteria

### Agent-verifiable

1. A rule set built from the backend's parameter names enables exactly the rules
   the backend enables for: all defaults; each rule's enabling parameter set
   alone; all eleven together; and parameters given as integers where floats are
   expected and the reverse. Enabled order and reported required columns equal
   the backend's for each case. This is unit tested against expectations copied
   from the backend's registry.
2. Unit tests state the evaluation rules as cases: state is discarded for a
   position that is no longer open; a reused identity starts fresh; the first
   firing rule wins and later rules are not updated on that bar; a missing
   required column skips the decision but not an independent update; missing
   high and low read the close; NaN comparisons and minima and maxima follow
   Python's operand rules.
3. Exit-rule reference fixtures exported from `q_backend`'s exit strategy at the
   existing reference pin pass the Q-021 gate under the exact policy, covering
   every scenario listed under Parity. Negative controls each fail: one expected
   state value moved by one ULP; one expected exit moved by one bar; one exit
   reason changed; a value that no longer matches its column checksum; and an
   unaccounted fixture file.
4. The Q-021 double-run determinism check passes for every exit scenario, and
   the prefix check passes at several prefix lengths per scenario.
5. The workspace dependency graph and the `forbid(unsafe_code)` and clippy
   determinism guards are unchanged for `q-engine`, and `make parity-isolation`
   passes.
6. The full validation suite passes: `make check`.

### Human-verifiable

1. The exit-rule fixtures are regenerated from the pinned backend in its full
   environment and show no diff. The time the regeneration takes is reported.
   Command: `cd q_core && time make fixtures-backend-check`
2. The rule catalog is reviewed against `q_backend`'s exit-rule registry at the
   pin: every registered rule, parameter name and default appears, and every
   deliberately reproduced quirk is named in the crate documentation.
   Command: `cd q_core && cargo doc -p q-engine --no-deps --open`
