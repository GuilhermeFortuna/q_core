# Q-083 implementation plan: Explainable market context kernels

> **For implementation agents:** Read the linked spec and repository instructions.
> Use superpowers:executing-plans when that skill is available. Start only through
> `./work start Q-083 --agent <agent> --worktree` after written-plan approval and
> completed dependencies. Implement this task natively; delegation requires separate authorization.

**Goal:** Produce deterministic, inspectable market-context readings from the existing price studies.
**Architecture:** q-indicators owns context math/categories; q_terminal owns target, freshness, coverage, history and text rendering.
**Spec:** [Specification](../specs/Q-083-explainable-market-context-kernels-spec.md)
**Status:** written plan awaiting human review.

## Global constraints

- The linked spec defines the interface, defaults and acceptance criteria; do not widen scope.
- Use the existing repository toolchain and canonical checks without resource-slice wrappers.
- Commit focused changes on the task branch. Never push, merge or change protected branches.
- Report unavailable prerequisites with the documented board workflow; do not substitute shortcuts.

## Ordered implementation

- [x] 1. Add crates/q-indicators/tests/context_analysis.rs with typed category/evidence fixtures, exact threshold boundaries, flat/mixed trends, warm-up, unavailable VWAP and session reset.
- [x] 2. Implement context.rs by composing the existing streaming states; separate pure classification of evidence from state advancement. Export the spec’s ContextConfig/Input/State/Output and reading types.
- [x] 3. Add batch_context driving commit, non-mutating preview and reset. Exercise every fixture prefix, repeated forming previews, invalid-config state preservation and causality/determinism helpers.
- [x] 4. Document defaults, evidence and category semantics in README.md. Run cargo test -p q-indicators --test context_analysis and make check; commit.
- [ ] 5. Hand off API and fixture outputs for normal human integration/release. Q-084 uses a published tag containing both Q-081 and Q-083; if Q-081 lands later, use that later release.

## Review focus

- RSI and volatility threshold equality are explicitly middle/typical; boundary fixtures assert this.
- A flat ATR denominator cannot divide to infinity; zero-baseline fixtures assert unavailable evidence.
- Forming slope compares with committed state; repeated-preview tests catch preview-to-preview drift.
- Missing VWAP does not erase other readings; partial-output fixture asserts independent statuses.
- Any future suffix cannot alter earlier context; prefix-causality tests exercise all categories.

## Validation and handoff

Run `cargo test -p q-indicators --test context_analysis` and `make check`. Existing fixtures and BACKEND_REV are unchanged; new context fixtures are hand-calculated.

Record acceptance results, exact dependency pins, any manual evidence and open follow-ups.
Commit the final changes, then run `./work board set Q-083 in-review -m "<changes; checks and results; follow-ups>"`
from the workspace root. The human owns integration and any required release.
