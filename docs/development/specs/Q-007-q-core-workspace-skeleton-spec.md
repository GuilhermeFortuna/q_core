# Q-007: `q_core` workspace skeleton

**Status:** authoritative in [`../STATUS.md`](../STATUS.md)  
**Project direction:** [`../../system-architecture.md`](../../system-architecture.md)  
**Depends on:** Q-006  
**Implementation plan:** [`../plans/Q-007-q-core-workspace-skeleton-plan.md`](../plans/Q-007-q-core-workspace-skeleton-plan.md)

## Purpose

Every semantic that backtesting and live execution both depend on must exist
exactly once, and the architecture puts that single copy in a Rust workspace
consumed by two very different hosts: a Python process through a native wheel,
and a Qt application through a C++ bridge. Neither integration is a detail that
can be deferred until there is something worth linking — a kernel ported into a
workspace whose wheel does not build, or whose crate boundaries turn out wrong,
is a kernel that has to be moved. This task creates the workspace, fixes the
crate boundaries and what each crate may depend on, and proves both binding
surfaces end to end with a trivial value, so that the first real kernel has
somewhere correct to land.

## Requirements

### Crate boundaries

- The workspace is divided into crates along the lines the architecture names,
  and each crate's responsibility is stated in the crate itself rather than only
  in a design document.
- The dependency direction between crates is declared and enforced by the build:
  a crate cannot depend on one above it, and an attempt to add such a dependency
  fails rather than working.
- The crate holding the Python binding contains binding code only and no
  semantics, so that the semantics remain reachable from the Qt host, which never
  loads Python.
- No crate depends on a host: nothing in the workspace knows whether it is being
  called from Python, from Qt, or from a test.

### Purity of the core

- The crates holding computational semantics perform no input or output, take no
  global configuration, read no environment, and know nothing of paths, catalogs,
  or retention.
- Determinism is a declared property: the same inputs produce the same outputs on
  the same platform, and the crates avoid the constructs that would make that
  untrue.
- The crate that reads columnar files reads the files it is handed and never
  discovers them, because discovery belongs to the process that owns dataset
  lifecycle.

### Python binding

- The workspace publishes a Python wheel, built by a declared tool, installable
  into the backend's environment.
- The wheel imports successfully in that environment and exposes at least one
  callable that returns a value derived from the Rust side, proving the boundary
  works rather than merely that the file was produced.
- The wheel's build is reproducible from a clean checkout with a single command.
- The Python-visible surface is named and typed such that a Python caller sees an
  ordinary module, not an artifact of the binding layer.

### Qt binding

- The workspace exposes a second binding surface for the Qt host, and the
  skeleton proves it compiles and links against the workspace.
- The Qt binding carries no semantics either; it is a projection of the same
  crates the wheel projects.
- Building the Qt binding does not require Python, and building the wheel does
  not require Qt, so that either host can be built alone.

### Contracts consumption

- The workspace carries the generated Rust types for the contracts commit it is
  pinned to, using the same vendoring protocol every other consumer uses.
- The carried code is verified against a clean regeneration in the workspace's
  CI, on the same terms as other consumers.

### Release

- The workspace is released by tagging, and the tag form is declared and is not a
  semantic version, because compatibility is decided by the parity suite rather
  than by a version number.
- Both consumers can pin to a tag: the Python side through its dependency
  declaration, the Qt side through its build manifest.
- The procedure for cutting a release is documented in the repository.

### Validation

- The workspace has a single documented command that formats, lints, builds, and
  tests everything including the wheel, and CI runs exactly that command.
- The lint configuration rejects the constructs that would undermine determinism
  or purity, so that the rules above are enforced rather than requested.

## Constraints and non-goals

- **No kernels.** No indicator, no candle loop, no tick loop, no fill model, no
  exit-rule state machine, no position sizing, no bar aggregation. The crates
  exist; they are empty of semantics. Porting the candle loop is the largest
  single item in the roadmap and it is not going to be smuggled into a skeleton.
- **No parity, determinism, or causality suites.** The architecture moves those
  suites into this repository and makes them the gate on every kernel. They
  arrive with the first kernel; a suite with nothing to test is a suite that will
  be written against nothing.
- **No changes to `q_backend`'s evaluation path.** The wheel is installed and
  imported to prove the boundary; no production code calls it.
- **No `q_terminal`.** The Qt binding is proven to compile; the application that
  uses it is Q-008.
- **No performance claims.** This task measures nothing and asserts nothing about
  speed. The comparison against the Python loop belongs to the task that replaces
  the Python loop.
- **No generic C ABI.** Two binding surfaces, each with a real consumer. A third
  is added when a third consumer exists.
- **No Arrow or Parquet integration beyond declaring the crate that will own it.**
  Reading real files is a later task with real files to read.

## Acceptance criteria

### Agent-verifiable

1. The workspace exists as an independent repository, builds from a clean
   checkout, and contains no path reference to a sibling checkout other than
   through the vendoring protocol's pinned fetch.
2. The declared crates exist, each with a stated responsibility in its own source.
3. The dependency direction is enforced: adding a dependency from a lower crate to
   a higher one fails the build, verified by making that change, observing the
   failure, and reverting it.
4. The binding crates contain no computational semantics, verified by a check that
   they expose only conversion and projection.
5. A wheel builds with a single command from a clean checkout.
6. The wheel installs into the backend's environment and a Python call returns a
   value produced on the Rust side, verified by a test.
7. The Qt binding compiles and links against the workspace, verified by a build
   that does not require Python.
8. The wheel builds without Qt present.
9. The workspace carries generated Rust contract types and a recorded contracts
   commit, and a clean regeneration produces no diff.
10. The lint configuration rejects at least the specific constructs named as
    determinism and purity hazards, verified by introducing one, observing the
    failure, and reverting it.
11. The release procedure is documented, and a tag of the declared form exists.
12. The full validation suite passes.

### Human-verifiable

1. A release is cut end to end: the tag is created, the backend's dependency is
   pointed at it, and the backend's environment resolves and installs the wheel
   from the tag.
   Command: `cd q_core && git tag vYYYY.MM.DD && cd ../q_backend && uv sync && uv run python -c "import q_core; print(q_core.version())"`
2. The crate boundaries are reviewed against §5 of the architecture and confirmed
   to place every named responsibility in exactly one crate, with no
   responsibility unplaced and none in two.
   Command: `$EDITOR q_core/README.md`
3. A clean-machine build is performed, confirming the documented prerequisites are
   complete: no step succeeds because of something already installed.
   Command: `podman run --rm -it -v $PWD:/src <clean image> sh -c 'cd /src && make check'`
