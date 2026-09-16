# Q-033 implementation plan: Columnar codecs for Arrow IPC and Parquet

**Status:** authoritative in the [Q project board](https://github.com/users/GuilhermeFortuna/projects/2)  
**Specification:** [`../specs/Q-033-columnar-codecs-for-arrow-ipc-and-parquet-spec.md`](../specs/Q-033-columnar-codecs-for-arrow-ipc-and-parquet-spec.md)  
**Depends on:** Q-025

## Current-system context

`crates/q-io/src/lib.rs` is 18 lines: `#![forbid(unsafe_code)]`, a doc comment
saying the crate holds "pure Parquet and Arrow columnar codecs and byte readers"
without filesystem discovery or lifecycle management, `CRATE_NAME`, and one
test. Its only dependency is `q-buffers`. Nothing in the workspace depends on
`q-io` except `q-qt`.

`crates/q-buffers` provides what the codecs produce. `Column` is
`Float64(Vec<f64>) | Int64(Vec<i64>) | Int8(Vec<i8>) | Bool(Bitmap)`;
`BarColumns` holds the contracted bar columns, `VolumeSet::from_bars` reports
which volume columns are present, `TimeLabel` carries the contract's time
marker, `BarFrame::try_new` validates and `push_column` adds extra columns,
`FrameError` is the crate's error type, and `RESERVED_COLUMNS` names the
contracted set. `BarFrame::schema` returns `FieldDesc` values whose type strings
are the contract's Arrow type strings.

`contracts/schema/api/arrow/bars.schema.json`, vendored by `make contracts` at
`CONTRACTS_REV` `998a5057`, declares `time timestamp[us]` with
`tz: "naive-wallclock-America/Sao_Paulo"`, `open`/`high`/`low`/`close` float64,
and `tick_volume`/`spread`/`real_volume` int64, all non-nullable. It is the
declared payload of `bars.forming` and `bars.completed` in
`schema/stream/topics.yaml`, and `schema/stream/framing.md` §2.2 puts the raw
Arrow IPC **stream** bytes after a length-prefixed JSON header in a binary
WebSocket frame.

`schema/catalog/dataset-manifest.schema.json` lists per file a lake-relative
`path`, `size_bytes` and a hex `checksum` under one manifest-wide
`checksum_algorithm` from `sha256 | sha512 | blake3 | md5`.
`q_backend/src/q_backend/market_data/catalog/repository.py:107` publishes
`checksum_algorithm="sha256"` and nothing else.

`q_backend`'s reads over the same files (Q-020) hand DuckDB an explicit file
list, never a directory or pattern, run with a UTC session time zone, and
tolerate partitions whose timestamps are nanoseconds or microseconds and whose
volume or spread columns are integer or nullable floating point.

The workspace lint table denies `float_cmp`, `float_cmp_const`,
`imprecise_flops` and `suboptimal_flops`; `clippy.toml` disallows `Instant`,
`SystemTime` and `env::var`/`var_os`. `make check` is
`fmt-check lint test fixtures-test fixtures-check parity-isolation wheel-test
qt-test contracts-check`. `make parity-isolation` fails if `q-parity` appears in
the `q-py` or `q-qt` dependency trees.

The gap is that `q_core` can compute over bars but cannot obtain them from
bytes, so both terminal data paths are blocked on this crate.

## Interfaces produced

```rust
// crates/q-io/src/error.rs
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IoError {
    UnexpectedColumn { name: String },
    MissingColumn { name: String },
    ColumnType { name: String, expected: &'static str, found: String },
    NullValue { file: Option<String>, column: String, row: usize },
    NotParquet { path: String },
    PathPattern { path: String },
    FileMissing { path: String },
    RowRange { start: usize, end: usize, rows: usize },
    UnsupportedDigest { algorithm: String },
    Arrow { detail: String },
    Frame(q_buffers::FrameError),
}
impl std::error::Error for IoError {}
impl std::fmt::Display for IoError { /* names the offending column, file or path */ }

// crates/q-io/src/arrow_ipc.rs
/// Decodes Arrow IPC *stream* bytes into the contracted bar columns.
/// Batches concatenate in order; zero batches give an empty frame.
pub fn decode_bar_batches(bytes: &[u8]) -> Result<BarFrame, IoError>;

/// The contracted bar field set the decoder and the Parquet reader both enforce,
/// in the order `BarFrame::schema` reports it.
pub fn bar_field_set() -> &'static [FieldDesc];

// crates/q-io/src/parquet.rs
/// A half-open positional row range across the listed files, counted as if they
/// were concatenated. `None` means every row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RowRange { pub start: usize, pub end: usize }

/// Reads the listed files, in order, into one bar frame. Opens exactly these
/// paths: no directory listing, no pattern expansion, no path derivation.
pub fn read_bar_files(paths: &[&Path], range: Option<RowRange>) -> Result<BarFrame, IoError>;

/// Rows a file holds, read from its footer without decoding a row group.
pub fn bar_file_rows(path: &Path) -> Result<usize, IoError>;

// crates/q-io/src/digest.rs
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DigestAlgorithm { Sha256 }
impl DigestAlgorithm {
    /// Parses a manifest's `checksum_algorithm`; anything else is
    /// `UnsupportedDigest`, so a caller fails closed.
    pub fn parse(name: &str) -> Result<Self, IoError>;
    pub fn name(self) -> &'static str;
}

/// Streams the file and returns the lowercase hex digest the manifest carries.
pub fn digest_file(path: &Path, algorithm: DigestAlgorithm) -> Result<String, IoError>;
```

## Implementation decisions

- **`arrow` and `parquet` from `arrow-rs`, pinned to one minor version, with
  default features off.** They are the only maintained Rust implementations of
  both formats, they share `arrow-schema`, and turning default features off
  keeps `chrono-tz`, `object_store` and the async stack out of a crate that must
  not know about clocks or networks. `#![forbid(unsafe_code)]` stays on `q-io`
  itself; the dependency's internal unsafe is not in this crate's scope, and no
  workspace rule forbids it.

- **Columns are matched by name and validated against `bar_field_set()`,
  not by position.** The stream's producer and the lake's writers order columns
  independently, and a positional decoder would silently transpose `spread` and
  `real_volume`, which have the same physical type. Name matching makes that a
  `MissingColumn` instead of wrong data.

- **Physical normalisation is allowed for Parquet and refused for Arrow IPC.**
  The lake holds files written years apart with different timestamp units and
  volume types, and refusing them would make history unreadable. The stream's
  payload is produced today, against a schema the topic policy names, so a
  mismatch there is a producer defect and must surface as one rather than be
  absorbed by a cast.

- **Timestamps normalise by flooring, not truncation.** Dividing a negative
  nanosecond timestamp with `/` truncates toward zero and moves a pre-epoch bar
  forward by up to a microsecond; `div_euclid` floors, which is what pandas and
  DuckDB do. Volume normalisation from float64 truncates toward zero instead,
  because that is what the tick aggregation (Q-029) records as today's
  behaviour and the two must not disagree.

- **Null checks run per column over the definition levels, not per value in the
  hot loop.** `arrow` already carries a null count per array; a zero null count
  skips the scan entirely, which is every file the backend writes. The row
  position in the error is resolved only on the failing column.

- **The reader streams row groups and appends into pre-sized column vectors.**
  The footer gives total rows before any decode, so each column is allocated
  once. This is what bounds peak memory to one row group and makes the memory
  test possible.

- **Pattern refusal is a character check on the path, not a filesystem probe.**
  `*`, `?`, `[`, `{` and a leading `~` are refused by name. A probe would be a
  time-of-check race and would also depend on the directory being listable,
  which §4.7 says a reader must not require.

- **Only sha256 is implemented, and every other declared algorithm is a named
  error.** The catalog publishes sha256 exclusively
  (`catalog/repository.py:107`). Implementing three unused digests would add
  three dependencies to justify a branch no manifest reaches, and a named error
  is what makes the terminal fail closed if that ever changes.

- **`RowRange` is positional and half-open across the concatenation.** A time
  filter would need the catalog's selection semantics, which §4.7 keeps out of
  `q_core`. Positional ranges are enough for "the last N bars" — the only thing
  the slice asks for — and compose with `bar_file_rows`.

## Ordered implementation

- [x] 1. Work on the branch `Q-033-columnar-codecs-for-arrow-ipc-and-parquet` in
   `q_core`, created from `development` by `./work start`. Confirm
   `CONTRACTS_REV` is `998a5057` and that `bars.schema.json` is unchanged
   between the pin and `q_contracts` `development`; otherwise stop and report.
- [x] 2. Add `arrow` and `parquet` with default features off, plus `sha2`, to
   `q-io`. Confirm `cargo tree -p q-py` and `cargo tree -p q-qt` still exclude
   `q-parity`, and that the wheel still builds. Commit.
- [x] 3. Write failing tests for `IoError`'s `Display`: each variant names its
   column, file, path or algorithm. Implement `error.rs`. Confirm they pass.
   Commit.
- [x] 4. Write failing tests in `digest.rs`: the sha256 vectors for the empty
   input and for a known small file; `parse("sha256")` succeeds;
   `parse("blake3")`, `parse("md5")` and `parse("SHA-256")` each give
   `UnsupportedDigest` naming the input. Implement. Confirm they pass. Commit.
- [x] 5. Write failing tests in `arrow_ipc.rs`, building batches with `arrow` in
   the test itself: a full bar batch decodes to the expected columns; two
   batches concatenate in order; an empty stream gives an empty frame; a
   reversed column order decodes identically; a renamed column gives
   `MissingColumn`; an added column gives `UnexpectedColumn`; float32 `close`
   and int32 `time` each give `ColumnType` naming the column; a null `high`
   gives `NullValue`; truncated bytes give `Arrow`. Implement
   `decode_bar_batches` and `bar_field_set`. Confirm they pass. Commit.
- [x] 6. Write failing tests in `parquet.rs` over files written by the test with
   `parquet`: two files concatenate in listed order; second, millisecond and
   nanosecond timestamp units floor to identical microseconds, including one
   pre-epoch value; float64 `tick_volume` of `2.9` and `-2.9` read as `2` and
   `-2`; a null `open` gives `NullValue` naming file, column and row; a missing
   path gives `FileMissing`; a text file gives `NotParquet`; `bars-*.parquet`
   gives `PathPattern`; a `RowRange` spanning the file boundary returns exactly
   its rows; a range past the end gives `RowRange`. Implement `read_bar_files`
   and `bar_file_rows`. Confirm they pass. Commit.
- [x] 7. Write a failing test that writes a file of eight row groups, reads it
   with an instrumented allocator, and asserts peak bytes beyond the frame stay
   under two row groups. Implement the streaming append if the test fails on
   memory rather than on values. Confirm it passes. Commit.
- [x] 8. Write a determinism test: decode and read each of the two paths twice in
   one process and compare the frames field for field. Confirm it passes.
   Commit.
- [x] 9. Add `bench-parquet-read` to the `Makefile`: read a lake range named by
   `Q_LAKE_RANGE`, five runs, reporting individual and median wall time and peak
   RSS, and compare the rows against the backend's DuckDB read at `BACKEND_REV`
   when `Q_BACKEND_CHECKOUT` is set. Commit.
- [x] 10. Run `make check`. Fix, re-run, commit.
- [ ] 11. **Human:** run `make bench-parquet-read` against the real lake and
   report the row comparison and the timings.

## Validation

- **Unit:** every Arrow rejection and acceptance case; every Parquet
  normalisation, rejection and range case; digest vectors and the unsupported
  algorithm; error display naming.
- **Integration:** a multi-row-group file read under the instrumented allocator.
- **Regression:** every gate merged before this task; the wheel builds and its
  surface is unchanged; `make parity-isolation`.
- **Determinism:** double decode and double read in one process.
- **Measurement:** throughput and peak RSS on a real lake range, compared row
  for row with the backend's DuckDB read.

```bash
cd /home/gui/projects/q/q_core
make check
cargo test -p q-io
cargo tree -p q-qt -e normal,build | grep -c q-parity   # expect 0

# human (step 11)
make bench-parquet-read
```

## Handoff

Report the pinned `arrow`, `parquet` and `sha2` versions and the feature sets
used. Report every physical type variation found in the real lake range and how
it normalised. Report the measured peak-memory ratio for the row-group test.
Report the DuckDB row comparison, the read timings, and whether `CONTRACTS_REV`
moved. State explicitly that no tick codec, no writer and no catalog logic
landed.
