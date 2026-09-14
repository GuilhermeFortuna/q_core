"""Wheel-level numpy round-trip and rejection tests for BarFrame / RollingBarWindow."""

from __future__ import annotations

import subprocess
import sys
import tempfile
from pathlib import Path


def main() -> None:
    root = Path(__file__).parents[1]
    dist = root / "dist"
    wheels = list(dist.glob("*.whl"))
    if not wheels:
        raise RuntimeError(f"No wheels found in {dist}. Run `make wheel` first.")
    wheel_path = sorted(wheels)[-1]

    with tempfile.TemporaryDirectory() as tmpdir:
        venv_dir = Path(tmpdir) / "venv"
        subprocess.check_call(["uv", "venv", str(venv_dir)], stdout=subprocess.DEVNULL)
        python_bin = venv_dir / "bin" / "python"
        subprocess.check_call(
            ["uv", "pip", "install", "--python", str(python_bin), str(wheel_path), "numpy"],
            stdout=subprocess.DEVNULL,
        )
        res = subprocess.run(
            [str(python_bin), "-c", CASES],
            capture_output=True,
            text=True,
        )
        if res.returncode != 0:
            sys.stderr.write(res.stdout)
            sys.stderr.write(res.stderr)
            sys.exit(res.returncode)
        print(res.stdout.strip())
        print("Bar frame wheel tests passed successfully.")


CASES = r"""
import numpy as np
import q_core

H = 3_600_000_000

# version unchanged
assert isinstance(q_core.version(), str)

# Round-trip from datetime64[ns] hourly times
times_ns = np.arange(
    np.datetime64("2023-01-02T00:00:00"),
    np.datetime64("2023-01-02T05:00:00"),
    np.timedelta64(1, "h"),
).astype("datetime64[ns]")
open_ = np.arange(5, dtype=np.float64)
high = open_ + 1.0
low = open_ - 1.0
close = open_ + 0.5

frame = q_core.BarFrame.from_numpy(times_ns, open_, high, low, close, tz="UTC")
assert len(frame) == 5
got_time = frame.column("time")
assert str(got_time.dtype) == "datetime64[us]"
assert np.array_equal(got_time.astype("datetime64[ns]"), times_ns)
assert frame.column("close").tobytes() == close.tobytes()

frame.add_column("buy_signal", np.array([True, False, True, False, True]))
assert frame.column("buy_signal").dtype == np.bool_
assert np.array_equal(frame.column("buy_signal"), np.array([True, False, True, False, True]))

frame.add_column("bar_index", np.arange(5))
assert frame.column("bar_index").dtype == np.int64
assert np.array_equal(frame.column("bar_index"), np.arange(5))

frame.add_column("dir", np.array([1, 0, -1, 0, 1], dtype=np.int8))
assert frame.column("dir").dtype == np.int8

# reject sub-us remainder
bad = np.array([0, 1], dtype="datetime64[ns]")
try:
    q_core.BarFrame.from_numpy(bad, np.zeros(2), np.zeros(2), np.zeros(2), np.zeros(2), tz="UTC")
    raise SystemExit("expected ValueError for sub-us time")
except ValueError as e:
    assert "time" in str(e)

# reject NaT
nat = np.array(["2023-01-02", "NaT"], dtype="datetime64[ns]")
try:
    q_core.BarFrame.from_numpy(nat, np.zeros(2), np.zeros(2), np.zeros(2), np.zeros(2), tz="UTC")
    raise SystemExit("expected ValueError for NaT")
except ValueError as e:
    assert "time" in str(e) or "NaT" in str(e)

# reject float16 open
try:
    q_core.BarFrame.from_numpy(
        times_ns,
        open_.astype(np.float16),
        high,
        low,
        close,
        tz="UTC",
    )
    raise SystemExit("expected TypeError for float16 open")
except TypeError as e:
    assert "open" in str(e)

# reject object bool column
try:
    frame.add_column("bad_bool", np.array([True, None, False, True, False], dtype=object))
    raise SystemExit("expected TypeError for object bool")
except TypeError as e:
    assert "bad_bool" in str(e)

# Rolling window scenario 2
w = q_core.RollingBarWindow(20, tz="UTC")
seed_t = (np.arange(30) * H).astype("datetime64[us]")
ones = np.ones(30, dtype=np.float64)
w.ingest_completed(seed_t, ones, ones * 2, ones * 0.5, ones * 1.5)
w.ingest_completed(
    np.array([30 * H], dtype="datetime64[us]"),
    np.array([1.0]),
    np.array([2.0]),
    np.array([0.5]),
    np.array([1.5]),
)
completed = w.completed()
assert len(completed) == 20
assert completed.column("time")[0].astype("datetime64[us]").astype(np.int64) == 11 * H
assert completed.column("time")[-1].astype("datetime64[us]").astype(np.int64) == 30 * H

print("all bar frame cases ok")
"""


if __name__ == "__main__":
    main()
