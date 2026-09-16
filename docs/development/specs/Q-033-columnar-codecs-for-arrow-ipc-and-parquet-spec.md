# Q-033: Columnar codecs for Arrow IPC and Parquet

**Status:** authoritative in the [Q project board](https://github.com/users/GuilhermeFortuna/projects/2)  
**Project direction:** [`q_contracts/docs/system-architecture.md` §4.6, §4.7, §5, §10 Phase 3](https://github.com/GuilhermeFortuna/q_contracts/blob/a9724767015905f1e9cd5d98ba492080910db4f2/docs/system-architecture.md#47-persistence-boundary-and-direct-parquet-access)  
**Depends on:** Q-025  
**Implementation plan:** [`../plans/Q-033-columnar-codecs-for-arrow-ipc-and-parquet-plan.md`](../plans/Q-033-columnar-codecs-for-arrow-ipc-and-parquet-plan.md)

## Purpose

The terminal slice reads bars from two byte sources: Arrow IPC batches arriving
as the binary half of a stream frame, and Parquet files a dataset manifest
lists. Architecture §4.7 puts both codecs in `q_core` and keeps catalogs, roots
and retention out of it — `q_core` reads the files it is handed. `q-io` is a
skeleton with one constant, so neither decode exists anywhere in Rust today, and
the two terminal tasks that need them would otherwise each grow their own. This
task implements both codecs over the bar columns Q-025 already defines, plus the
digest the catalog's reader verification needs, so that every later consumer
takes bytes in and a bar frame out.

## Requirements

### Arrow IPC decode

- A byte slice in Arrow IPC **stream** format decodes into a bar frame whose
  columns are the ones the bars Arrow schema declares: a microsecond timestamp,
  four float64 prices, and three int64 volume-class columns, none of them
  nullable.
- A stream carrying several record batches decodes into one frame, in batch
  order. A stream carrying none decodes into an empty frame with the same
  columns.
- A missing column, an extra column, a column of the wrong physical type, or a
  null in any column is an error that names the column and what was found. No
  value is cast, defaulted or dropped to make a batch fit.
- Column order in the payload does not matter; columns are matched by name.
- Timestamps decode as the integers they are. The lake's naive Brasília
  wall-clock convention is a property of the data, and the decoder neither
  converts nor interprets it.
- Decoding the same bytes twice gives equal frames, and decoding never depends on
  the machine's time zone or locale.

### Parquet read

- An explicit, ordered list of file paths reads into one bar frame, rows in
  listed-file order and, within a file, in stored order. The reader opens exactly
  the files it is given.
- The reader never lists a directory, expands a wildcard, or derives a path from
  another path. A path that a filesystem or query layer could read as a pattern
  is refused with an error naming it.
- A file that is absent, unreadable, or not Parquet is an error naming the file.
  Nothing is skipped silently.
- The lake's physical variation is accepted and normalised: a timestamp column
  stored in seconds, milliseconds, microseconds or nanoseconds normalises to
  microseconds by flooring, and a volume-class column stored as float64
  normalises to int64 by truncation toward zero. Any other physical type for a
  contracted column is an error naming the file, the column and the type.
- A null in a contracted column is an error naming the file, the column and the
  row position, because the contract declares none of them nullable.
- Reading is bounded: peak memory beyond the resulting frame stays within one row
  group, whatever the file's size.
- A caller may restrict the read to a half-open row range across the listed
  files, so that a consumer can read a dataset's tail without reading its head.

### Digests

- The bytes of a file digest under the algorithms the catalog declares, returning
  the lowercase hexadecimal form the manifest carries.
- An algorithm the implementation does not support is an error naming it, so a
  caller fails closed rather than treating an unverifiable file as verified.
- Digesting streams the file and does not hold it in memory.

### Boundaries preserved

- `q-io` gains no notion of a catalog, a dataset identity, a lake root, a
  retention policy, or a network. It takes paths and bytes.
- The crate stays free of unsafe code, clocks, and environment access, and every
  error it returns is a typed value rather than a panic. No input aborts the
  process.
- Every gate merged before this task keeps passing unchanged, the wheel still
  builds without Qt, and `q-parity` stays out of the `q-py` and `q-qt`
  dependency trees.

## Constraints and non-goals

- **No change to any other crate's public API.** Q-025's frame and column types
  are used as they are; if a codec needs something they do not offer, it is
  built inside `q-io`.
- **No tick payloads.** The quotes topic carries the ticks schema, and the slice
  renders bars. A tick codec lands with its first consumer.
- **No writing.** Nothing in `q_core` writes Parquet or Arrow. Dataset
  publication is `q_backend`'s, and stays there.
- **No file discovery, no manifest parsing, no checksum policy.** The reader
  receives a list; deciding what to list, and what a failed digest means, is
  Q-036's.
- **No predicate pushdown, projection pushdown, or time filtering.** The row
  range is positional. A time window is the catalog's selection, made before the
  read.
- **No Python or Qt exposure.** The wheel's surface is unchanged; the Qt
  projection is Q-034.
- **No compression or encoding beyond what the lake's files already use.**

## Acceptance criteria

### Agent-verifiable

1. Unit tests state the Arrow rules as cases: a well-formed batch round-trips to
   the expected columns; batches concatenate in order; an empty stream gives an
   empty frame; a renamed column, a float32 price, an int32 timestamp, a null
   value and an extra column each raise an error naming the column; column order
   in the payload does not change the result.
2. Unit tests state the Parquet rules as cases: files concatenate in listed
   order; second-, millisecond- and nanosecond-stored timestamps floor to the
   same microsecond values; a float64 volume truncates toward zero, including a
   negative value; a null raises an error naming file, column and row; a missing
   file, a non-Parquet file and a path containing a glob character each raise an
   error naming the path; a row range returns exactly its rows across a file
   boundary.
3. A test reads a file larger than one row group and asserts the read holds no
   more than one row group of decoded bytes beyond the frame.
4. Digest tests cover a known vector per supported algorithm, an empty file, and
   an unsupported algorithm named in the error.
5. Decoding and reading the same input twice in one process gives bit-identical
   frames.
6. `q-io` has no unsafe code, no clock and no environment access, and its
   dependency direction is unchanged.
7. The full validation suite passes: `make check`.

### Human-verifiable

1. On the real lake, one month of M1 bars for the live symbol reads through the
   Parquet path and is compared row for row against the same range read by the
   backend's DuckDB path. Any difference is reported with its cause.
   Command: `cd q_core && make bench-parquet-read`
2. Read throughput and peak resident memory are reported for that same range,
   five runs, individual and median.
   Command: `cd q_core && make bench-parquet-read`
