# Q-111 implementation plan: Research indicator kernels

> **For implementation agents:** Read the linked spec and repository instructions.
> Use superpowers:executing-plans when that skill is available. Start only through
> `./work start Q-111 --agent <agent> --worktree` after written-plan approval.
> Implement this task natively; delegation requires separate authorization.

**Goal:** `q_core.indicators` offers log returns, per-bar volatility estimators, volatility-normalized scores, a log-price z-score, a volatility-regime ratio and bar-aggregated quoted spread.
**Architecture:** new modules in `q-indicators` beside the pandas-parity kernels, which stay untouched; `q-py` projects each function.
**Spec:** [Specification](../specs/Q-111-research-indicator-kernels-spec.md)
**Status:** written plan awaiting human review.

## Global constraints

- The linked spec defines the formulas, conventions and acceptance criteria; do not widen scope.
- The 16 existing kernels, `window.rs`, the reference fixtures and `BACKEND_REV` are not edited. New code may call `atr`, `rolling_mean` and `rolling_std`.
- Keep the crate's purity rules: no I/O, clock, environment or unsafe code; no fused multiply-add.
- Use the repository toolchain and canonical checks without resource-slice wrappers.
- Commit focused changes on the task branch. Never push, merge or change protected branches.

## Ordered implementation

- [ ] 1. Add `crates/q-indicators/tests/research_kernels.rs` with criteria 1 to 9 and 12. Confirm it fails for the expected reason.
- [ ] 2. Add `crates/q-indicators/src/research.rs`: a shared log-close helper that maps invalid prices to NaN, the half-life recursion with its `N_EW` readiness and restart, then the kernels of the spec table. Export them from `lib.rs`. Run the test file; commit.
- [ ] 3. Add `crates/q-indicators/tests/quote_spread.rs` with criteria 10 and 11, then `crates/q-indicators/src/quotes.rs` with `quote_spread_bars` as a single pass over ticks and bars. Commit.
- [ ] 4. Project every kernel in `crates/q-py/src/indicators.rs` and register it. Add cases to `tests/test_wheel.py` for criterion 13.
- [ ] 5. Document the conventions, the kernel table and the quote rules in `README.md`. Run `make check`. Commit.

## Review focus

- First-valid indices match the spec table exactly, including the off-by-one between returns and closes.
- An invalid bar restarts an exponentially weighted kernel and its readiness count, and does not leak the old state.
- `price_zscore_log` and the two quotient kernels apply the `1e-12` rule to the volatility, not to the result.
- The quote pass clips each carried interval at the next update, the expiry, the session change and the bar close, and never counts a zero-duration quote.
- Equal-time ticks are applied in source order; the last one defines the carried quote.
- No existing fixture, tolerance or kernel body changed.

## Validation and handoff

Run `cargo test -p q-indicators --test research_kernels --test quote_spread`, then `make check`. Record results and follow-ups,
commit, then run `./work board set Q-111 in-review -m "<changes; checks and results; follow-ups>"` from the
workspace root. The human owns integration and the release; Q-112 pins the published tag.
