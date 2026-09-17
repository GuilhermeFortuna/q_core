"""Benchmark Parquet read throughput and compare row for row against DuckDB (Q-033).

Five runs over a lake range named by Q_LAKE_RANGE, reporting individual and median
wall time and peak RSS, and comparing against backend DuckDB read when Q_BACKEND_CHECKOUT is set.
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).parents[1]
BACKEND_CHECKOUT = os.environ.get("Q_BACKEND_CHECKOUT")
LAKE_RANGE = os.environ.get("Q_LAKE_RANGE")


def _resolve_paths() -> list[Path]:
    if LAKE_RANGE:
        paths = []
        for part in LAKE_RANGE.replace(":", ",").split(","):
            part = part.strip()
            if part:
                p = Path(part)
                if not p.is_absolute():
                    p = (ROOT / p).resolve()
                paths.append(p)
        if paths:
            return paths

    # Default candidates
    candidates = [
        ROOT.parent / "q_contracts" / "tests" / "fixtures" / "bars_sample.parquet",
        ROOT.parent / "q_backend" / "tests" / "fixtures" / "lake" / "ohlcv" / "WIN$N" / "M15" / "2025.parquet",
        ROOT / "contracts" / "tests" / "fixtures" / "bars_sample.parquet",
    ]
    for c in candidates:
        if c.exists():
            return [c.resolve()]

    return []


def _compare_duckdb(paths: list[Path], rust_data: dict, backend_dir: Path) -> None:
    try:
        import duckdb
    except ImportError:
        venv_python = backend_dir / ".venv" / "bin" / "python"
        if venv_python.exists():
            cmd = [
                str(venv_python),
                "-c",
                (
                    "import sys, json, duckdb, numpy as np; "
                    "dump = json.loads(open(sys.argv[1]).read()); "
                    "paths = [str(p) for p in sys.argv[2:]]; "
                    "con = duckdb.connect(); con.execute(\"SET GLOBAL TimeZone = 'UTC'\"); "
                    "query = 'SELECT epoch_us(CAST(time AS TIMESTAMP)) AS time, open, high, low, close, "
                    "CAST(tick_volume AS BIGINT) AS tick_volume, CAST(spread AS BIGINT) AS spread, "
                    "CAST(real_volume AS BIGINT) AS real_volume "
                    "FROM read_parquet(?, union_by_name=true, filename=true, file_row_number=true) "
                    "ORDER BY list_position(?, filename), file_row_number'; "
                    "df = con.execute(query, [paths, paths]).df(); "
                    "assert len(df) == dump['rows'], f'Row count mismatch: {len(df)} vs {dump[\"rows\"]}'; "
                    "np.testing.assert_array_equal(df['time'].to_numpy(), np.array(dump['time'])); "
                    "np.testing.assert_allclose(df['open'].to_numpy(), np.array(dump['open'])); "
                    "np.testing.assert_allclose(df['high'].to_numpy(), np.array(dump['high'])); "
                    "np.testing.assert_allclose(df['low'].to_numpy(), np.array(dump['low'])); "
                    "np.testing.assert_allclose(df['close'].to_numpy(), np.array(dump['close'])); "
                    "if dump['tick_volume']: np.testing.assert_array_equal(df['tick_volume'].to_numpy(), np.array(dump['tick_volume'])); "
                    "if dump['spread']: np.testing.assert_array_equal(df['spread'].to_numpy(), np.array(dump['spread'])); "
                    "if dump['real_volume']: np.testing.assert_array_equal(df['real_volume'].to_numpy(), np.array(dump['real_volume'])); "
                    "print(f'DuckDB row comparison: PASS ({len(df)} rows compared against DuckDB at {sys.argv[2]}, 0 differences)')"
                ),
                sys.argv[1] if len(sys.argv) > 1 else "",
                *[str(p) for p in paths],
            ]
            # Write dump to temp file
            with tempfile.NamedTemporaryFile("w", suffix=".json", delete=False) as f:
                json.dump(rust_data, f)
                tmp_json = f.name
            cmd[3] = tmp_json
            try:
                subprocess.check_call(cmd)
            finally:
                Path(tmp_json).unlink(missing_ok=True)
            return
        else:
            print("Warning: DuckDB not available in current environment; skipping row comparison.")
            return

    import numpy as np

    con = duckdb.connect()
    con.execute("SET GLOBAL TimeZone = 'UTC'")
    path_strs = [str(p) for p in paths]
    query = """
    SELECT
        epoch_us(CAST(time AS TIMESTAMP)) AS time,
        open,
        high,
        low,
        close,
        CAST(tick_volume AS BIGINT) AS tick_volume,
        CAST(spread AS BIGINT) AS spread,
        CAST(real_volume AS BIGINT) AS real_volume
    FROM read_parquet(?, union_by_name=true, filename=true, file_row_number=true)
    ORDER BY list_position(?, filename), file_row_number
    """
    df = con.execute(query, [path_strs, path_strs]).df()
    assert len(df) == rust_data["rows"], f"Row count mismatch: DuckDB {len(df)} vs Rust {rust_data['rows']}"
    np.testing.assert_array_equal(df["time"].to_numpy(), np.array(rust_data["time"]))
    np.testing.assert_allclose(df["open"].to_numpy(), np.array(rust_data["open"]))
    np.testing.assert_allclose(df["high"].to_numpy(), np.array(rust_data["high"]))
    np.testing.assert_allclose(df["low"].to_numpy(), np.array(rust_data["low"]))
    np.testing.assert_allclose(df["close"].to_numpy(), np.array(rust_data["close"]))
    if rust_data["tick_volume"]:
        np.testing.assert_array_equal(df["tick_volume"].to_numpy(), np.array(rust_data["tick_volume"]))
    if rust_data["spread"]:
        np.testing.assert_array_equal(df["spread"].to_numpy(), np.array(rust_data["spread"]))
    if rust_data["real_volume"]:
        np.testing.assert_array_equal(df["real_volume"].to_numpy(), np.array(rust_data["real_volume"]))

    print(f"DuckDB row comparison: PASS ({len(df)} rows compared against DuckDB, 0 differences)")


def main() -> None:
    bin_path = ROOT / "target" / "release" / "bench-parquet-read"
    if not bin_path.exists():
        subprocess.check_call(["cargo", "build", "--release", "-p", "q-io", "--bin", "bench-parquet-read"], cwd=str(ROOT))

    paths = _resolve_paths()
    with tempfile.NamedTemporaryFile("r", suffix=".json", delete=False) as f:
        dump_json = Path(f.name)

    try:
        cmd = [str(bin_path), "--dump-json", str(dump_json)]
        if paths:
            cmd.extend(str(p) for p in paths)
        subprocess.check_call(cmd, cwd=str(ROOT))

        if BACKEND_CHECKOUT:
            backend_dir = Path(BACKEND_CHECKOUT)
            if backend_dir.exists():
                with open(dump_json, "r", encoding="utf-8") as f:
                    data = json.load(f)
                resolved_paths = [Path(p) for p in data.get("paths", [str(p) for p in paths])]
                print()
                _compare_duckdb(resolved_paths, data, backend_dir)
            else:
                print(f"Note: Q_BACKEND_CHECKOUT={BACKEND_CHECKOUT} does not exist; skipping DuckDB row check.")
    finally:
        dump_json.unlink(missing_ok=True)


if __name__ == "__main__":
    main()
