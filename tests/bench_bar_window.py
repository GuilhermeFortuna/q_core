"""Compare RollingBarWindow ingest cost vs pandas window update.

Five runs of 10_000 one-bar ingests into a full 605-bar window.
Not part of make check.
"""

from __future__ import annotations

import statistics
import time

import numpy as np
import pandas as pd
import q_core


BOUND = 605
RUNS = 5
INGESTS = 10_000
H = 3_600_000_000


def _seed_arrays(n: int):
    times = (np.arange(n) * H).astype("datetime64[us]")
    open_ = np.ones(n, dtype=np.float64)
    high = open_ * 2
    low = open_ * 0.5
    close = open_ * 1.5
    return times, open_, high, low, close


def bench_rust() -> list[float]:
    times, open_, high, low, close = _seed_arrays(BOUND)
    results = []
    for _ in range(RUNS):
        w = q_core.RollingBarWindow(BOUND, tz="UTC")
        w.ingest_completed(times, open_, high, low, close)
        t0 = time.perf_counter()
        for i in range(INGESTS):
            t = np.array([(BOUND + i) * H], dtype="datetime64[us]")
            o = np.array([1.0])
            h = np.array([2.0])
            l = np.array([0.5])
            c = np.array([1.5])
            w.ingest_completed(t, o, h, l, c)
        elapsed = time.perf_counter() - t0
        results.append(elapsed / INGESTS * 1e6)
    return results


def bench_pandas() -> list[float]:
    idx = pd.date_range("2023-01-02", periods=BOUND, freq="h", tz="UTC")
    frame = pd.DataFrame(
        {
            "open": 1.0,
            "high": 2.0,
            "low": 0.5,
            "close": 1.5,
        },
        index=idx,
    )
    results = []
    for _ in range(RUNS):
        rolling = frame.copy()
        next_t = idx[-1] + pd.Timedelta(hours=1)
        t0 = time.perf_counter()
        for i in range(INGESTS):
            batch = pd.DataFrame(
                {"open": [1.0], "high": [2.0], "low": [0.5], "close": [1.5]},
                index=[next_t + pd.Timedelta(hours=i)],
            )
            ordered = batch.sort_index()
            merged = pd.concat([rolling, ordered])
            merged = merged[~merged.index.duplicated(keep="last")].sort_index()
            rolling = merged.iloc[-BOUND:]
        elapsed = time.perf_counter() - t0
        results.append(elapsed / INGESTS * 1e6)
    return results


def main() -> None:
    rust = bench_rust()
    pandas = bench_pandas()
    print("RollingBarWindow us/ingest:", ", ".join(f"{x:.3f}" for x in rust))
    print("RollingBarWindow median:", f"{statistics.median(rust):.3f}")
    print("pandas us/ingest:", ", ".join(f"{x:.3f}" for x in pandas))
    print("pandas median:", f"{statistics.median(pandas):.3f}")


if __name__ == "__main__":
    main()
