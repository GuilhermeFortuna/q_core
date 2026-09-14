# Q-021: Reference fixtures and parity gate

**Status:** authoritative in the [Q project board](https://github.com/users/GuilhermeFortuna/projects/2)  
**Project direction:** [`q_contracts/docs/system-architecture.md` §7, §9 invariant 1, §10 phase 2](https://github.com/GuilhermeFortuna/q_contracts/blob/f2273a88e52c5b9a8ad5ac9d7eb27f8069cf643d/docs/system-architecture.md#7-repositories)  
**Depends on:** Q-007  
**Implementation plan:** [`../plans/Q-021-reference-fixtures-and-parity-gate-plan.md`](../plans/Q-021-reference-fixtures-and-parity-gate-plan.md)

## Purpose

The architecture moves the golden, determinism, and causality suites into
`q_core` and makes them the gate on every kernel. Today those suites exist only
in `q_backend`, in Python, and they test pandas code. `q_core` has no record of
what the Python implementation returns, and it has no way to check a Rust
kernel against that record. The first kernel port, Q-022's 16 indicator and
transform functions, would otherwise be checked against numbers its author
chose. This task exports reference fixtures from today's Python implementation
at a pinned `q_backend` commit and commits them to `q_core` with their
provenance. It adds a reusable Rust harness for golden comparison, double-run
determinism, and prefix causality, and each check has a negative control that
proves it can fail. The indicator fixtures arrive pending, and the gate says
exactly which kernel turns each one on. Q-022 then has a fixed target, and
Q-025 and the batch 05 kernels reuse the same gate.

## Requirements

### Reference fixtures for indicators and transforms

- There is one reference fixture for each of the 16 functions Q-022 ports: the
  7 functions in the backend's technical indicators module (realized
  volatility, Yang–Zhang volatility, RSI, Bollinger bands, MACD, Donchian
  channels, ATR), each of the 5 moving-average types (SMA, EMA, SMMA, WMA,
  HMA), and the 4 transforms (rolling z-score, rolling rank, percent change,
  clip).
- Expected outputs are what the backend's Python implementation returns at the
  pinned commit. Nothing is computed a second way, and nothing is corrected
  where the Python behaves surprisingly. Rolling z-score uses sample standard
  deviation despite its docstring, Donchian period 0 returns all missing
  values, and the fixture records both.
- The main input is the fixed-seed synthetic OHLCV series that the backend's
  golden backtest tests generate, with that generator's defaults. The fixture
  carries the series values, so no reader has to rerun the generator.
- Each function is also exercised on inputs chosen to hit edge behavior: a
  constant series, a strictly increasing series, a series with missing values
  (at the start, in a run, and isolated), and a series shorter than the
  windows used.
- Parameters include the common values, the smallest valid window, windows
  longer than the series, and parameters for which the Python implementation
  raises. A raising case is recorded as a rejection, not left out.
- Every case with a window has a warm-up region of missing values in its
  expected output, and the position of each missing value is part of the
  reference.
- Every output of multi-output functions is recorded under its own name:
  Bollinger upper, middle, and lower; MACD line, signal, and histogram; Donchian
  upper and lower.

### Provenance and reproducibility

- Every fixture records the `q_backend` repository and commit it was exported
  from, the exact source file version of each function and of the generator,
  the exporter that wrote it, the numerical library versions it ran under,
  and the CPU feature level of the machine that ran it.
- The pinned commit is reachable in the published `q_backend` repository. A
  sibling checkout is never needed.
- Regenerating from a clean `q_core` checkout with one command reproduces the
  committed fixtures byte for byte. Running the exporter twice gives identical
  bytes, so a fixture holds no timestamp, host name, or other run-dependent
  value.
- The exporter runs under the same numerical library versions as the
  backend's locked environment at the pinned commit, and it refuses to run
  under any other versions.
- The exporter refuses to export from a reference module that imports anything
  other than the standard library and the numerical libraries. The reference
  must be today's self-contained Python, and never a later backend that
  delegates to `q_core`, which would make the gate compare `q_core` with itself.
- Before writing a function's fixture, the exporter checks that the reference
  itself is prefix-causal on the fixture inputs, using the backend's own
  causality assertion. A non-causal reference is never committed as truth.

### Fixture format

- Fixtures are text that a reviewer can read and diff. A change to one
  expected value shows as a change to one line.
- Values are stored exactly. Every float64 input and output read back from a
  fixture has the bits the exporter produced. Every stored column carries a
  checksum of those bits, so a reader that parses a value inexactly fails
  instead of passing with a nearby value.
- Missing values, positive infinity, negative infinity, and negative zero are
  each represented without loss and without non-standard text.
- Reading fixtures from Rust needs no dependency the workspace does not already
  carry, and nothing heavier than a JSON parser.
- The format carries a version identifier. A reader rejects a version it does
  not know.
- The format supports integer columns as well as float64 columns, so later
  fixture families, such as Q-025's bar times, use the same reader.

### Fixture families

- Fixtures are grouped into named families under one fixture root, and the
  indicator family is the first. A later family adds its own export
  scenario, its own case layout, and its own gate. It reuses the shared
  envelope, provenance, column encoding, comparison policies, checks, and
  pending bookkeeping. It does not add a second format or a second exporter.
- A family's cases need not be a function of input series. A family whose
  cases are sequences of operations with an expected state after each step,
  as Q-025's rolling-window scenarios are, can be hosted without changing the
  indicator family's files.
- Each family declares which reference environment it needs. Some families
  read only self-contained numerical modules and run in a minimal environment
  with the backend's locked numerical library versions. Others drive real
  backend objects, such as the `StrategyEvaluator` Q-025 drives, and run in the
  backend's own full locked environment at the pinned commit. The exporter
  refuses to run a family in an environment it did not declare.

### Comparison policy

- Each fixture declares the comparison policy it is judged by. The policy is
  not chosen by the test that runs it, so loosening a policy shows up as a
  fixture change.
- Missing-value positions must match exactly under every policy. Warm-up
  placement is semantics, not rounding.
- Infinities must match exactly, including sign, under every policy.
- A tolerance policy accepts a finite value when its difference from the
  expected value is within an absolute bound, or within a relative bound
  scaled by the expected value's magnitude. Either bound is enough. Negative
  zero equals zero, as in the backend's golden normalization.
- The indicator fixtures use an absolute bound at the backend golden files'
  resolution of 10 decimal places, and a relative bound of one part in 10^12.
  Some references use a dot product whose summation order depends on the CPU,
  so a correct kernel can differ from them by an amount that grows with the
  value's magnitude. An absolute bound alone would fail correct kernels on
  real-scale prices, and the relative bound alone would loosen the check on
  small values. With both, small values are still held to 10 decimal places.
- An exact policy is also available, in which values must be bit-identical, for
  fixtures that copy values instead of computing them. Integer columns are
  always compared exactly.
- A failed comparison names the function, the case, the output, the first
  differing index, and the expected and actual values.

### Determinism and causality checks

- A double-run check runs a kernel twice on independently allocated copies of
  the same inputs. Any difference in the result bits fails, and there is no
  tolerance.
- A prefix-causality check recomputes a kernel on every prefix of each case's
  inputs. A value at bar `t` that differs from the full-series value at bar
  `t` fails. The check truncates all input columns together and checks every
  output.
- The checks are usable with any kernel through a small adapter. They are not
  tied to the indicator fixtures.

### Negative controls

- Each check is shown to fail against a deliberately broken kernel, in the
  standard test run and not only by hand. The broken kernels are a value
  perturbed beyond tolerance, a moved missing value, an infinity of the wrong
  sign, an output that changes between calls, a kernel that reads the next
  bar, and a kernel that accepts a parameter the reference rejects.
- A forward-looking kernel whose expected outputs are themselves forward-looking
  passes the golden comparison and still fails causality. This proves that
  causality is checked independently of the goldens.

### Pending fixtures and the gate

- Every committed fixture is either bound to a kernel or listed as pending, and
  the gate fails on a fixture that is neither. No fixture is ever silently
  skipped.
- A function that is both bound and still listed as pending fails the gate.
  Turning a fixture on therefore means binding it and removing its pending entry
  in the same change.
- A pending entry that names no fixture fails the gate, so the list cannot go
  stale.
- A bound kernel is judged on every case: goldens under the fixture's policy,
  rejection wherever the reference rejected, double-run determinism, and prefix
  causality.
- With no kernels, which is this task's final state, the gate passes and reports
  16 pending and 0 bound.
- The gate fails if any fixture's recorded backend commit differs from the
  commit `q_core` pins, so a pin change without regeneration is caught offline.

### Staleness

- The CPU feature level is part of provenance, because a vector math library
  can return a result one ULP different on a wider instruction set. When
  the staleness check runs on a machine whose level differs from the
  recorded one, it says so. Everything except values must still match
  byte for byte, and values must match under each fixture's own policy,
  with inputs always exact. A level difference alone is therefore never
  reported as staleness.
- CI regenerates the minimal-environment families from the pinned commit and
  fails on any difference from the committed files. This catches hand edits, a
  changed exporter without regeneration, and a changed pin.
- The standard validation command runs that staleness check, as it already
  runs the contracts drift check.
- Families that need the backend's full environment have a separate, named
  staleness check that does the same comparison in that environment. It is
  documented as required before any change to those families or to the pin
  is merged. It is not part of the standard validation command, because that
  environment is several gigabytes. The offline commit comparison in the gate
  still covers these families in every test run.

### Build independence and preserved guarantees

- `q_core` builds and its Rust tests run from a clean checkout with no network
  access. Only the staleness check and regeneration reach the published
  `q_backend` repository.
- The harness is test support. It is never linked into the Python wheel or the
  Qt library, and a build fails if either binding crate gains a normal
  dependency on it.
- The semantic crates contain no kernels after this task. Their purity lint
  rules and unsafe-code prohibition are unchanged, and the harness follows the
  same rules.
- The existing validation steps keep running and passing unchanged: format,
  lint, tests, wheel build and integration test, Qt harness, and contracts
  drift check.
- `q_backend` is not modified.

## Constraints and non-goals

- **No kernels.** No indicator, transform, or moving average is implemented in
  Rust, not even the elementwise clip, and not even as a test-only positive
  control. The functions are Q-022's, and a Rust clip written here to show the
  gate end to end would be a kernel that no one reviewed as a kernel.
- **No numpy projection and no Python surface.** The wheel's surface is
  unchanged. Q-022 decides how kernels appear in Python.
- **No `q_backend` change.** The exporter reads the backend at a pinned
  commit. Adding an export command to the backend is tempting, because it
  would let the export import the backend normally, but it would couple a
  `q_core` gate to a backend release and create a second copy of the
  generator's call site.
- **No backtest, rolling-window, tick, exit-rule, or sizing fixtures.** Q-025
  exports its rolling-window scenarios through this exporter and gate, and the
  batch 05 tasks export engine fixtures through them. This task provides the
  format, reader, checks, gate, and family mechanism, and fills in only the
  indicator family. It also does not decide whether CI runs the
  full-environment staleness check. The first family that needs it, Q-025,
  makes that decision against its measured cost.
- **No backtest↔live parity suite.** The architecture ports it to `q_core`
  with the evaluator, which is Q-031. Nothing here compares a backtest with a
  live evaluation.
- **No automatic tracking of `q_backend` development.** The pin moves only as a
  deliberate change that shows as a fixture diff. After Q-023 the backend's
  indicators delegate to `q_core`, and the pin must never be moved past that
  point for this family.
- **No change to the backend's golden files or their normalization.** The
  gate's tolerance is declared to match their resolution, not replace it.
- **No performance measurement of kernels.** There are no kernels to measure.
  The only measured costs are the exporter's run time and the staleness check's
  run time.
- **No Parquet, Arrow IPC, or NumPy binary fixtures.** Each would give exact
  values cheaply, but none is reviewable as a diff, and the first two would pull
  a columnar IO stack into the test path ahead of `q-io`'s phase 3 task.

## Acceptance criteria

### Agent-verifiable

1. Reference fixtures exist for exactly the 16 named functions. Each covers the
   synthetic OHLCV series and every edge input. Each has at least one case with
   a leading warm-up region of missing values and at least one case with a
   window longer than its input. The case count per function and the total
   fixture size are reported.
2. Every fixture records the pinned `q_backend` commit, and that commit is
   fetched from the published repository by commit hash. Every fixture also
   records the source file version of its function and of the generator, the
   exporter identity, the numpy and pandas versions, and the CPU feature
   level. The library versions equal the backend's locked versions at that
   commit.
3. Regenerating from a clean checkout produces byte-identical fixtures, and two
   consecutive regenerations are byte-identical to each other.
4. The staleness check passes on the committed fixtures. It fails after one
   expected value is hand-edited, and after the pin is changed to a different
   commit without regeneration. Both are reverted.
5. The exporter refuses to run when a numerical library version differs from the
   backend lock. It also refuses a reference module that imports a
   non-standard, non-numerical module. Both are verified by tests.
6. Reading every fixture from Rust reproduces every stored column's checksum. A
   control with one flipped value bit fails the checksum and names the column.
7. For every committed fixture, comparing the stored expected outputs with
   themselves passes. A value perturbed by twice the larger of the two bounds
   fails with the function, case, output, index, and both values in the
   message. A perturbation of half the larger bound passes. A difference that
   exceeds both bounds fails. A difference above the absolute bound that is
   within the relative bound of a large expected value passes. A moved
   missing value fails, a sign-flipped infinity fails, and negative zero
   against zero passes. Under the exact policy, a one-ULP change fails and
   negative zero against zero fails.
8. The double-run check passes a stable kernel and fails a kernel whose output
   changes between calls.
9. The causality check passes a causal kernel. It fails a next-bar kernel whose
   golden comparison passes and names the first failing index.
10. A kernel that returns values for a case the reference rejected fails the
    gate, and so does a kernel that errors on a case the reference computed.
11. With no kernels bound, the gate passes and reports 16 pending and 0 bound. It
    fails when one pending entry is removed, when a pending entry names an
    unknown function, when a bound function is still pending, and when a
    fixture's recorded commit differs from the pin.
12. The Rust test suite passes with the network unavailable.
13. Neither binding crate has a normal or build dependency on the harness. A
    check fails when such a dependency is added, and the change is reverted.
14. The family mechanism is proven without a second real family. A test-only
    family whose cases are operation sequences with integer and float64
    columns under the exact policy loads through the shared reader, and its
    pending bookkeeping passes and fails as the indicator gate's does. The
    exporter refuses to run a family declared for the full backend
    environment when it is started in the minimal environment.
15. The full validation suite passes, including the staleness check.

### Human-verifiable

1. Fixtures exported in `q_backend`'s own full locked environment at the pinned
   commit are byte-identical to the committed fixtures. This confirms that the
   minimal exporter environment does not change a single value.
   Command: `git -C q_backend worktree add /tmp/qb-ref "$(cat q_core/BACKEND_REV)" && uv sync --frozen --project /tmp/qb-ref && uv run --frozen --project /tmp/qb-ref python q_core/tools/reference/export_reference.py --backend-checkout /tmp/qb-ref --family indicators --allow-numeric-in-backend --out /tmp/qb-fixtures && diff -ru q_core/fixtures/reference /tmp/qb-fixtures`
2. A reviewer opens the RSI fixture and, without other documentation, finds its
   provenance, the warm-up region of the period-14 synthetic case, the
   strictly increasing case's value of 100, and the period-0 rejection. The
   reviewer confirms the file is readable as a diff.
   Command: `less q_core/fixtures/reference/indicators/rsi.json`
3. The task branch's CI run on GitHub passes, including the staleness check
   against the published `q_backend` repository. The wall-clock time of the
   staleness step is reported.
   Command: `gh run watch -R GuilhermeFortuna/q_core`
