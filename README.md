# `q_core`

Deterministic computational core for the Q trading platform.

`q_core` provides a single authoritative implementation of every market, simulation, and execution semantic that research backtesting and live trading depend on. It is consumed by two hosts:
- Python (`q_backend` and execution workers) via a native wheel built with PyO3/maturin.
- Qt/QML (`q_terminal`) via a native bridge built with CXX-Qt.

Neither host embeds computational semantics; all deterministic logic lives in Rust within this workspace.

---

## Crate Responsibilities

Every responsibility named in §5 of the system architecture is mapped to exactly one crate:

| Crate | Responsibility | Role & Boundary |
| --- | --- | --- |
| `q-indicators` | Indicator mathematics | Pure technical indicators and streaming state machines over numerical series. Zero I/O, zero system clock or environment access. |
| `q-engine` | Simulation & execution kernels | Candle and tick kernels, fill model, exit-rule state machines, position sizing. Shared identically by backtesting and live evaluation. |
| `q-buffers` | Columnar memory buffers & contracts | Columnar ring buffers, streaming bar windows, Arrow memory layouts, and vendored contract types (`contracts/`). |
| `q-io` | Columnar codecs & readers | Parquet and Arrow codecs and byte decoders. Reads explicitly provided files/buffers; never performs filesystem discovery. |
| `q-py` | Python native binding | PyO3 projection layer. Exposes `q_core` module and callables to Python. Projection only; contains zero computational semantics. |
| `q-qt` | Qt native binding | CXX-Qt projection layer. Exposes `CoreInfo` and QObject interfaces to Qt/QML. Projection only; contains zero computational semantics. |

---

## Dependency Direction

Dependencies flow strictly downward and are enforced by Cargo manifests:

```
q-indicators  ───►  (none)
q-buffers     ───►  contracts module
q-io          ───►  q-buffers
q-engine      ───►  q-indicators, q-buffers
q-py          ───►  q-engine, q-io, q-buffers, q-indicators
q-qt          ───►  q-engine, q-io, q-buffers, q-indicators
```

- Any attempt to introduce an upward or cyclic dependency (such as `q-indicators -> q-engine`) fails the build at `cargo metadata` / compile time.
- Neither binding crate (`q-py`, `q-qt`) depends on the other. Building `q-py` requires no Qt toolchain; building `q-qt` requires no Python headers.
- No semantic crate depends on a host or knows whether it runs in Python, Qt, or unit tests.

---

## Determinism & Purity Guards

1. **`#![forbid(unsafe_code)]`**:
   Enforced in all four semantic crates (`q-indicators`, `q-engine`, `q-buffers`, `q-io`). Unsafe code is only permitted in the two binding crates (`q-py`, `q-qt`) where FFI boundaries require it.
2. **Disallowed Non-Deterministic APIs**:
   The shared workspace Clippy configuration denies `std::time::Instant`, `std::time::SystemTime`, `std::time::Instant::now`, `std::time::SystemTime::now`, `std::env::var`, and `std::env::var_os`.
3. **Floating-Point Comparison**:
   Direct floating-point equality (`==` and `!=`) is denied by `clippy::float_cmp` and `clippy::float_cmp_const`.

---

## Contracts Vendoring Protocol

`q_core` vendors generated Rust types from `q_contracts`:
- `CONTRACTS_REV`: Commit hash of `q_contracts` pinned by this workspace.
- `contracts/`: Vendored generated Rust types (`api.rs`, `catalog.rs`, `edge.rs`, `mod.rs`, `stream.rs`), included by `q-buffers`.
- `make contracts`: Clones `q_contracts` at `CONTRACTS_REV` and copies generated Rust files.
- `make contracts-check`: Clones `q_contracts` at `CONTRACTS_REV`, regenerates Rust contracts from schemas, and asserts zero diff.

---

## Prerequisites

Building and testing `q_core` requires:
- **Rust Toolchain**: 1.98.0 (pinned via `rust-toolchain.toml`), with `rustfmt` and `clippy`.
- **C++ Compiler & Build Tools**: `g++` (C++17 support), `make`, `git`, `curl` (`build-essential` on Debian/Ubuntu).
- **Python Toolchain**: Python 3.12+ and `uv` (for maturin release wheel builds and isolated virtualenv integration tests).
- **Qt 6 Runtime Libraries**: If a system Qt 6 SDK is not installed, `qt-build-utils` / `cxx-qt` automatically downloads a minimal Qt 6 core runtime (`qt_minimal`). The downloaded runtime dynamically links:
  - On Debian/Ubuntu: `libglib2.0-0t64` (or `libglib2.0-0`), `libdouble-conversion3`, and `libpcre2-16-0`.

---

## Development and Validation

The canonical check command runs format verification, linting, tests, wheel building, wheel integration test, Qt harness test, and contracts check:

```bash
make check
```

Individual targets:
```bash
make fmt-check        # Verify rustfmt formatting
make lint             # Run clippy with -D warnings
make test             # Run cargo test across workspace
make wheel            # Build release wheel with maturin
make wheel-test       # Test installing and importing wheel in clean venv
make qt-test          # Build q-qt static library and run C++ test harness
make contracts-check  # Verify vendored contracts match clean regeneration
```
