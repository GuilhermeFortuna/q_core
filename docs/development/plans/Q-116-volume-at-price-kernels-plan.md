# Q-116 implementation plan: Volume-at-price kernels

> **For implementation agents:** Read the linked spec and repository instructions.
> Use superpowers:executing-plans when that skill is available. Start only through
> `./work start Q-116 --agent <agent> --worktree` after written-plan approval.
> Implement this task natively; delegation requires separate authorization.

**Goal:** `q_core.indicators` computes volume at price per segment, with aggressor split, point of control and value area, and reports developing levels at checkpoints.
**Architecture:** a new `vap.rs` module in `q-indicators` that reuses the Q-081 side classification; `q-py` projects both functions.
**Spec:** [Specification](../specs/Q-116-volume-at-price-kernels-spec.md)
**Status:** written plan awaiting human review.

## Global constraints

- The linked spec defines the formulas, conventions and acceptance criteria; do not widen scope.
- `volume.rs`, the existing kernels, `window.rs`, the reference fixtures and `BACKEND_REV` are not edited. `vap.rs` may import `AggressorSide`, `BUY_FLAG` and `SELL_FLAG`.
- Keep the crate's purity rules: no I/O, clock, environment or unsafe code; no fused multiply-add.
- Rows are keyed by integer row index (`BTreeMap<i64, _>` or an equivalent ordered structure), never by float price.
- Use the repository toolchain and canonical checks without resource-slice wrappers.
- Commit focused changes on the task branch. Never push, merge or change protected branches.

## Ordered implementation

- [x] 1. Add `crates/q-indicators/tests/volume_at_price.rs` with criteria 1 to 8. Confirm it fails for the expected reason.
- [x] 2. Add `crates/q-indicators/src/vap.rs`: input validation, the trade rule and row binning, a per-profile row accumulator, and one `poc_and_value_area` function shared by both kernels. Implement `volume_at_price` as one pass over ticks and segments. Export from `lib.rs`. Commit.
- [x] 3. Implement `volume_at_price_levels` on the same accumulator, resetting on session change and evaluating each checkpoint before applying ticks at or after it. Commit.
- [x] 4. Project both kernels in `crates/q-py/src/indicators.rs` and register them. Add cases to `tests/test_wheel.py` for criterion 9. Commit.
- [x] 5. Document the row grid, trade rule, POC tie-break and value-area expansion in `README.md`. Run `make check`. Commit.

## Review focus

- `round_half_even` and the floor division of negative tick indices are exact; a row is never chosen by float comparison.
- The value-area loop walks a dense ladder between the lowest and highest traded rows and terminates when `value_area = 1`.
- A trade at a segment close or at a checkpoint time falls on the documented side.
- The developing kernel's final checkpoint matches the segment kernel bit for bit.
- No existing fixture, tolerance or kernel body changed.

## Validation and handoff

Run `cargo test -p q-indicators --test volume_at_price`, then `make check`. Record results and follow-ups,
commit, then run `./work board set Q-116 in-review -m "<changes; checks and results; follow-ups>"` from the
workspace root. The human owns integration and the release; Q-117 pins the published tag.
