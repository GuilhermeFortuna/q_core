# Q-081 implementation plan: Deterministic volume analysis kernels

> **For implementation agents:** Read the linked spec and repository instructions.
> Use superpowers:executing-plans when that skill is available. Start only through
> `./work start Q-081 --agent <agent> --worktree` after written-plan approval and
> completed dependencies. Implement this task natively; delegation requires separate authorization.

**Goal:** Provide shared deterministic calculations for tape-derived volume studies.
**Architecture:** q-indicators owns volume arithmetic and side semantics; q_terminal owns source coverage and deduplication.
**Spec:** [Specification](../specs/Q-081-deterministic-volume-analysis-kernels-spec.md)
**Status:** written plan awaiting human review.

## Global constraints

- The linked spec defines the interface, defaults and acceptance criteria; do not widen scope.
- Use the existing repository toolchain and canonical checks without resource-slice wrappers.
- Commit focused changes on the task branch. Never push, merge or change protected branches.
- Report unavailable prerequisites with the documented board workflow; do not substitute shortcuts.

## Ordered implementation

- [ ] 1. Add crates/q-indicators/tests/volume_analysis.rs with hand-computed buy/sell/unknown, session reset, rate-boundary and threshold-equality fixtures; include identical timestamp multiplicity.
- [ ] 2. Implement crates/q-indicators/src/volume.rs and public types/exports. Compose batch processing from VolumeState; keep clock/session/coverage policy outside the crate.
- [ ] 3. Add prefix parity, double-run determinism, rejected-input state preservation and event-capacity tests. Use explicit checkpoints to test no-trade window ageing.
- [ ] 4. Document volume semantics, units supplied by callers and bounded-memory errors in README.md. Run cargo test -p q-indicators --test volume_analysis and make check; commit.
- [ ] 5. Hand off the public API and test outputs. Request normal human integration/release through ./work finish; Q-082 must pin that published tag.

## Review focus

- Unknown side is not inferred from price direction; side fixtures assert unknown quantity separately.
- Equal trades at equal timestamps are counted twice; multiplicity fixture checks every field.
- A quiet market ages the rate window through supplied time; advance tests avoid a hidden clock.
- Invalid/capacity input cannot partially increment CVD; clone/state-output tests prove preservation.
- A new session clears both CVD and activity state; reset fixtures assert no prior-day leakage.

## Validation and handoff

Run `cargo test -p q-indicators --test volume_analysis` then `make check`. Fixtures are new hand-calculated cases; existing reference outputs and BACKEND_REV remain unchanged.

Record acceptance results, exact dependency pins, any manual evidence and open follow-ups.
Commit the final changes, then run `./work board set Q-081 in-review -m "<changes; checks and results; follow-ups>"`
from the workspace root. The human owns integration and any required release.
