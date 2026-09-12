# Q-007 implementation plan: `q_core` workspace skeleton

**Status:** authoritative in [`../STATUS.md`](../STATUS.md)  
**Specification:** [`../specs/Q-007-q-core-workspace-skeleton-spec.md`](../specs/Q-007-q-core-workspace-skeleton-spec.md)  
**Depends on:** Q-006

## Current-system context

There is no Rust in the project outside `q_frontend/src-tauri/`, which is a Tauri
shell and is not a workspace anything else consumes. The semantics this workspace
will eventually hold are all in Python today: the per-bar loop in
`q_backend/src/q_backend/backtesting/engine.py`, the `numba` tick kernel in
`backtesting/tick/kernel.py`, and — on the execution side — `execution/evaluator.py`,
`execution/signal_eval.py`, `execution/indicator_frame.py`, `execution/bars.py`
and `execution/parity.py`, the last of which exists precisely because the
backtest and live paths must agree and currently do so by test rather than by
construction. `q_backend` already depends on `pyarrow>=24.0.0` and `numba>=0.65.1`,
and its `[tool.uv.sources]` already demonstrates the pattern for pointing a
dependency at a non-registry source.

After Q-006, `q_contracts` emits Rust into `generated/rust/` as a source tree with
no `Cargo.toml`, and both existing consumers carry a `contracts/` directory, a
`CONTRACTS_REV` file, and `make contracts` / `make contracts-check` targets that
clone the pinned commit into a temp directory. `COMPAT.md` records the pin set.
The gap this task closes is that there is no Rust workspace for any of it to land
in, and no proof that either binding surface — PyO3 to the backend, `cxx-qt` to
the terminal — actually works on this machine before a kernel depends on both.

## Interfaces produced

```
// q_core/  (new repository)
Cargo.toml              workspace manifest; resolver 2; shared lint table
rust-toolchain.toml     exact toolchain pin
crates/
  q-indicators/         indicator math                      (empty of semantics)
  q-engine/             candle + tick kernels, fills, exits, sizing (empty)
  q-buffers/            columnar ring buffers, Arrow buffers (empty)
  q-io/                 Parquet/Arrow codecs and readers     (empty)
  q-py/                 PyO3 binding; projection only
  q-qt/                 cxx-qt binding; projection only
contracts/              vendored generated Rust; included as a module by q-buffers
CONTRACTS_REV
Makefile                `make check`, `make wheel`, `make contracts`, `make contracts-check`
RELEASING.md            the tag procedure
README.md               crate responsibilities and the dependency direction
```

```rust
// crates/q-indicators/src/lib.rs   — and the same shape in q-engine, q-buffers, q-io
#![forbid(unsafe_code)]
//! Responsibility: <one paragraph, stated here and not only in README.md>

/// The crate's build identity. The only public item until the first kernel lands.
pub const CRATE_NAME: &str = "q-indicators";
```

```rust
// crates/q-py/src/lib.rs   — projection only
#[pymodule]
fn q_core(m: &Bound<'_, PyModule>) -> PyResult<()>;

/// Workspace version, read from the workspace manifest at compile time.
#[pyfunction] fn version() -> &'static str;

/// The contracts commit this build vendored, read from CONTRACTS_REV at compile time.
#[pyfunction] fn contracts_rev() -> &'static str;
```

```rust
// crates/q-qt/src/lib.rs   — projection only
#[cxx_qt::bridge]
mod ffi {
    extern "RustQt" {
        #[qobject] #[qproperty(QString, version)]
        #[qproperty(QString, contracts_rev)]
        type CoreInfo = super::CoreInfoRust;
    }
}
```

```
// dependency direction, enforced by the workspace manifest
q-indicators  ->  (nothing)
q-buffers     ->  contracts module
q-io          ->  q-buffers
q-engine      ->  q-indicators, q-buffers
q-py          ->  q-engine, q-io, q-buffers, q-indicators
q-qt          ->  q-engine, q-io, q-buffers, q-indicators
```

## Implementation decisions

- **Six crates, not the five §7 names.** The architecture lists `q-indicators`,
  `q-engine`, `q-buffers`, `q-io`, and `q-py`, leaving the `cxx-qt` binding
  unplaced. Putting it in `q-py` would mean the terminal's build pulls PyO3 and a
  Python interpreter; putting it in `q-engine` would mean the wheel's build pulls
  Qt. `q-qt` as a sibling of `q-py` is what makes acceptance criteria 7 and 8 —
  each host building without the other's toolchain — achievable at all.

- **`q-buffers` owns the vendored contracts module rather than a crate of its
  own.** Q-006 deliberately emits a source tree with no manifest so that including
  it does not introduce a path dependency. `q-buffers` is the lowest crate that
  needs payload shapes, and attaching the module there means `q-io` and `q-engine`
  get them by their existing dependency edge rather than by a new one.

- **The dependency direction is enforced by the manifests themselves, not by a
  lint.** Cargo already fails on a dependency cycle, and the six manifests are the
  declaration; there is no second rule file to keep in sync. Criterion 3 is
  verified by adding the illegal edge and observing cargo's own error, which is the
  strongest available form of "enforced by the build".

- **`#![forbid(unsafe_code)]` in the four semantic crates, permitted only in the
  two binding crates.** Unsafe code is how a deterministic kernel acquires
  undefined behavior that reproduces on one machine and not another, which would
  destroy the parity suite's value before it is written. The bindings genuinely
  need it; the semantics genuinely do not, and forbidding is cheaper than
  reviewing.

- **The lint table lives in the workspace manifest and is inherited, not copied
  into six crates.** Six copies drift, and the drift is silent: one crate quietly
  permits what the others forbid, and it is discovered when a kernel is moved into
  it. The specific denials are floating-point comparison by `==`, `f32`/`f64`
  transcendental use where a deterministic alternative exists, iteration over
  hash-ordered collections in output paths, and any use of the system clock or
  environment in the semantic crates — each one a way a "deterministic" result
  becomes platform- or run-dependent.

- **`version()` and `contracts_rev()` are the proof surface, chosen because they
  are non-forgeable from the host side.** A binding that returns a constant the
  Python side could have known anyway proves only that a module loaded. Reading
  the workspace version from the Cargo manifest and the contracts hash from
  `CONTRACTS_REV` at compile time means the returned values can only have come
  through the boundary, and the same two values appear on the Qt side, so a
  mismatch between the two hosts is immediately visible.

- **The Qt binding exposes properties on a QObject rather than invokable
  methods.** QML binds to properties declaratively, and the first real use — a
  status line showing which core build the terminal is running — is a binding, not
  a call. Starting with a method would mean the first real surface changes shape.

- **`maturin` builds the wheel, and the wheel is consumed by `q_backend` through a
  git dependency on a tag rather than through a local index.** §7.1 allows either;
  a local index is a second piece of infrastructure to keep alive on one developer
  machine, and its absence is discovered at the worst time. A git tag needs
  nothing that is not already there. The cost is a longer `uv sync` on first
  resolution, which is measured and reported rather than assumed.

- **Tags are `vYYYY.MM.DD[.n]` and explicitly not semantic versions.** A semantic
  version makes a promise about compatibility that only the parity suite can
  actually check, and the suite does not exist yet. A date tag makes no promise,
  which is honest, and `COMPAT.md` carries the real compatibility statement.

- **`make check` builds the wheel as part of the standard suite, rather than only
  in a release job.** The wheel build is the step most likely to break from an
  unrelated change — a new dependency that does not cross-compile, a toolchain
  bump — and discovering that at release time, when the release is the thing being
  attempted, is the worst moment.

- **The clean-machine build is a human acceptance step, not a CI job.** CI images
  accumulate exactly the prerequisites that make a documented-prerequisites claim
  untestable. A container run from a bare image, performed by hand once, is what
  actually verifies that `README.md` is complete.

## Ordered implementation

1. Create the branch `Q-007-q-core-workspace-skeleton-spec` in a newly initialized
   `q_core` repository at `/home/gui/projects/q/q_core`.
2. Add the workspace `Cargo.toml` with resolver 2, the six members, the shared
   `[workspace.lints]` table, and `rust-toolchain.toml` pinning an exact toolchain.
   Add `.gitignore`. Confirm `cargo metadata` succeeds. Commit.
3. Create the four semantic crates, each with `#![forbid(unsafe_code)]`, a
   responsibility paragraph in its `lib.rs`, `CRATE_NAME`, and one unit test
   asserting `CRATE_NAME` equals the crate's package name. Write the tests first
   and confirm they fail to compile before the constant exists. Wire the
   dependency edges listed above. Confirm `cargo test --workspace` passes. Commit.
4. Add `CONTRACTS_REV` with the hash from `COMPAT.md`, a `Makefile` with
   `contracts` and `contracts-check` targets identical in behavior to the other
   consumers', and run `make contracts` to populate `contracts/`. Include it as a
   module from `q-buffers`. Confirm `cargo test --workspace` and
   `make contracts-check` both pass. Commit.
5. Add `q-py` with PyO3, `version()`, `contracts_rev()`, and the `q_core` module.
   Read the workspace version and the contracts hash at compile time from the
   manifest and the file, not as literals. Add `pyproject.toml` for `maturin` and
   a `make wheel` target. Confirm `make wheel` produces a wheel from a clean
   `cargo clean`. Commit.
6. Write a failing integration test that installs the built wheel into a temporary
   virtual environment and asserts `q_core.version()` equals the workspace version
   parsed from `Cargo.toml` and `q_core.contracts_rev()` equals the content of
   `CONTRACTS_REV`. Confirm it fails before the wheel is wired, then confirm it
   passes. Commit.
7. Add `q-qt` with `cxx-qt`, the `CoreInfo` QObject exposing the same two values as
   properties, and a build that produces a static library. Add a minimal C++ test
   harness that instantiates `CoreInfo` and asserts both properties are non-empty
   and that `version` matches the workspace version. Write the harness assertion
   first and confirm it fails against an unimplemented property. Commit.
8. Verify criteria 7 and 8 explicitly: build `q-qt` in an environment with no
   Python development headers and confirm it succeeds; build the wheel in an
   environment with no Qt and confirm it succeeds. Record both commands and
   outcomes. Commit any manifest feature-gating the verification required.
9. Verify criterion 3: add a dependency from `q-indicators` to `q-engine`, run
   `cargo metadata`, confirm it fails naming the cycle, and revert. Do not commit
   the illegal edge.
10. Verify criterion 10: introduce a float `==` comparison in `q-engine`, run
    `make check`, confirm the lint fails naming the construct, and revert. Do not
    commit it.
11. Write `README.md` mapping each responsibility named in §5 of the architecture
    to exactly one crate, and stating the dependency direction. Write
    `RELEASING.md` with the tag form, the steps to cut a release, and how each
    consumer pins to it. Commit.
12. Add `.github/workflows/ci.yml` running `make check`, which formats, clips,
    tests the workspace, builds the wheel, runs the wheel integration test, builds
    `q-qt`, and runs `make contracts-check`. Commit.
13. Update `q_contracts/COMPAT.md` with the `q_core` repository and its first tag,
    and the "verified by" line naming the commands run here.
14. Human step, matching human-verifiable criterion 1: cut the tag, point
    `q_backend`'s dependency at it, run `uv sync`, and import the module.
15. Human step, matching human-verifiable criterion 2: review the crate boundaries
    against §5 and confirm every named responsibility is in exactly one crate.
16. Human step, matching human-verifiable criterion 3: run `make check` inside a
    bare container and confirm the documented prerequisites are complete.
17. Run the full validation suite and commit. Report the handoff.

## Validation

- **Unit:** `CRATE_NAME` per crate; the C++ harness assertions on `CoreInfo`.
- **Integration:** the wheel installed into a temporary environment, with both
  Python-visible values compared against their on-disk sources — this is the only
  test that proves the PyO3 boundary rather than the build.
- **Regression:** none yet. This task establishes the baseline the parity and
  determinism suites will be added to when the first kernel lands.
- **Manual:** steps 14, 15, and 16; plus the two negative verifications in steps 9
  and 10, which are run by hand because they require introducing a defect.
- **Measurement:** report the wall-clock time of a clean `make check` and of the
  first `uv sync` in `q_backend` resolving the git-tagged wheel, because the
  latter is the cost the git-over-index decision accepted and it should be a
  number rather than a guess.

```bash
cd /home/gui/projects/q/q_core
make contracts-check
make check                       # fmt, clippy, cargo test --workspace, wheel, q-qt, wheel integration test
make wheel

# criterion 3, the illegal dependency edge
cargo add --package q-indicators --path crates/q-engine 2>&1 | tail -5   # expect failure
git checkout -- crates/q-indicators/Cargo.toml

# criterion 1, the release round trip
git tag v2026.09.12
cd /home/gui/projects/q/q_backend && uv sync && uv run python -c "import q_core; print(q_core.version(), q_core.contracts_rev())"
```

## Handoff

Report the six crate names with the one-line responsibility each states in its own
source, and the dependency edges as cargo resolved them — this is the boundary
every later kernel is placed against and it should be reported as fact, not as
intent. Report the values returned by `q_core.version()` and
`q_core.contracts_rev()` from the installed wheel alongside the workspace version
in `Cargo.toml` and the content of `CONTRACTS_REV`, so the boundary proof is shown
rather than asserted, and report the same two values as read from the Qt side's
`CoreInfo`. Report the exact cargo error from the illegal dependency edge in step
9 and the exact lint error from step 10, since those two messages are the evidence
that the rules are enforced rather than documented. Report the two cross-toolchain
builds from step 8 with the commands used and confirmation that neither host's
toolchain was present. Report the clean `make check` wall-clock time and the
`uv sync` resolution time. Report the tag created and confirm `COMPAT.md` now
lists four repositories.
