#!/usr/bin/env python3
"""Compare pandas (q_backend) vs q_core.indicators timings for selected kernels.

Default length is 1_000_000 bars (100_000 for rolling_rank). Pass ``--n N`` or
set ``COMPARE_PANDAS_N`` for a shorter smoke run.
"""

from __future__ import annotations

import argparse
import os
import statistics
import sys
import time

import numpy as np
import pandas as pd

from q_backend.backtesting.moving_averages import compute_ma
from q_backend.backtesting.technical_indicators import (
    compute_bollinger_bands,
    compute_rsi,
    compute_yang_zhang,
)
from q_backend.backtesting.transforms import compute_rolling_rank

import q_core.indicators as qi

ABS_TOL = 1e-10
REL_TOL = 1e-12
RUNS = 5
SEED = 42


def agree(actual: np.ndarray, expected: np.ndarray, name: str) -> None:
    if actual.shape != expected.shape:
        raise AssertionError(f"{name}: shape {actual.shape} != {expected.shape}")
    for i, (a, e) in enumerate(zip(actual, expected, strict=True)):
        if np.isnan(e) and np.isnan(a):
            continue
        if np.isnan(e) or np.isnan(a):
            raise AssertionError(f"{name}: NaN placement mismatch at {i}")
        if np.isinf(e) or np.isinf(a):
            if np.isinf(e) and np.isinf(a) and np.sign(e) == np.sign(a):
                continue
            raise AssertionError(f"{name}: infinity mismatch at {i}")
        diff = abs(float(a) - float(e))
        if diff <= ABS_TOL or diff <= REL_TOL * abs(float(e)):
            continue
        raise AssertionError(
            f"{name}: mismatch at {i}: actual={a} expected={e} abs_diff={diff}"
        )


def median_ns(samples: list[int]) -> int:
    return int(statistics.median(samples))


def time_calls(label: str, fn, runs: int = RUNS) -> list[int]:
    samples: list[int] = []
    for i in range(runs):
        t0 = time.perf_counter_ns()
        fn()
        dt = time.perf_counter_ns() - t0
        samples.append(dt)
        print(f"  {label} run {i + 1}: {dt} ns")
    print(f"  {label} median: {median_ns(samples)} ns")
    return samples


def make_series(n: int) -> tuple[pd.Series, pd.Series, pd.Series, pd.Series, np.ndarray]:
    rng = np.random.default_rng(SEED)
    close_np = 100.0 + np.cumsum(rng.normal(0.0, 0.5, size=n))
    open_np = close_np + rng.normal(0.0, 0.1, size=n)
    high_np = np.maximum(open_np, close_np) + rng.uniform(0.0, 0.3, size=n)
    low_np = np.minimum(open_np, close_np) - rng.uniform(0.0, 0.3, size=n)
    idx = pd.RangeIndex(n)
    return (
        pd.Series(open_np, index=idx),
        pd.Series(high_np, index=idx),
        pd.Series(low_np, index=idx),
        pd.Series(close_np, index=idx),
        close_np.astype(np.float64, copy=False),
    )


def parse_n(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--n",
        type=int,
        default=None,
        help="Series length override (also COMPARE_PANDAS_N)",
    )
    args = parser.parse_args(argv)
    if args.n is not None:
        return args.n
    env = os.environ.get("COMPARE_PANDAS_N")
    if env:
        return int(env)
    return 1_000_000


def main(argv: list[str] | None = None) -> int:
    n = parse_n(argv)
    rank_n = min(n, 100_000) if n >= 100_000 else n
    if n == 1_000_000:
        rank_n = 100_000
    print(f"n={n} rolling_rank_n={rank_n} seed={SEED} runs={RUNS}")

    open_s, high_s, low_s, close_s, close_np = make_series(n)
    open_np = open_s.to_numpy(dtype=np.float64)
    high_np = high_s.to_numpy(dtype=np.float64)
    low_np = low_s.to_numpy(dtype=np.float64)

    # RSI(14)
    pandas_rsi = compute_rsi(close_s, 14).to_numpy(dtype=np.float64)
    core_rsi = np.asarray(qi.rsi(close_np, 14), dtype=np.float64)
    agree(core_rsi, pandas_rsi, "rsi")
    print("rsi")
    time_calls("pandas", lambda: compute_rsi(close_s, 14))
    time_calls("q_core", lambda: qi.rsi(close_np, 14))

    # Bollinger(20, 2)
    pu, pm, pl = compute_bollinger_bands(close_s, 20, 2.0)
    cu, cm, cl = qi.bollinger_bands(close_np, 20, 2.0)
    agree(np.asarray(cu), pu.to_numpy(dtype=np.float64), "bollinger.upper")
    agree(np.asarray(cm), pm.to_numpy(dtype=np.float64), "bollinger.middle")
    agree(np.asarray(cl), pl.to_numpy(dtype=np.float64), "bollinger.lower")
    print("bollinger_bands")
    time_calls("pandas", lambda: compute_bollinger_bands(close_s, 20, 2.0))
    time_calls("q_core", lambda: qi.bollinger_bands(close_np, 20, 2.0))

    # WMA(20)
    pandas_wma = compute_ma(close_s, 20, "wma").to_numpy(dtype=np.float64)
    core_wma = np.asarray(qi.wma(close_np, 20), dtype=np.float64)
    agree(core_wma, pandas_wma, "wma")
    print("wma")
    time_calls("pandas", lambda: compute_ma(close_s, 20, "wma"))
    time_calls("q_core", lambda: qi.wma(close_np, 20))

    # HMA(20)
    pandas_hma = compute_ma(close_s, 20, "hma").to_numpy(dtype=np.float64)
    core_hma = np.asarray(qi.hma(close_np, 20), dtype=np.float64)
    agree(core_hma, pandas_hma, "hma")
    print("hma")
    time_calls("pandas", lambda: compute_ma(close_s, 20, "hma"))
    time_calls("q_core", lambda: qi.hma(close_np, 20))

    # Yang–Zhang(20)
    pandas_yz = compute_yang_zhang(open_s, high_s, low_s, close_s, 20).to_numpy(
        dtype=np.float64
    )
    core_yz = np.asarray(qi.yang_zhang(open_np, high_np, low_np, close_np, 20), dtype=np.float64)
    agree(core_yz, pandas_yz, "yang_zhang")
    print("yang_zhang")
    time_calls(
        "pandas",
        lambda: compute_yang_zhang(open_s, high_s, low_s, close_s, 20),
    )
    time_calls(
        "q_core",
        lambda: qi.yang_zhang(open_np, high_np, low_np, close_np, 20),
    )

    # rolling_rank(20) on shorter series
    _, _, _, close_rank_s, close_rank_np = make_series(rank_n)
    pandas_rank = compute_rolling_rank(close_rank_s, 20).to_numpy(dtype=np.float64)
    core_rank = np.asarray(qi.rolling_rank(close_rank_np, 20), dtype=np.float64)
    agree(core_rank, pandas_rank, "rolling_rank")
    print("rolling_rank")
    time_calls("pandas", lambda: compute_rolling_rank(close_rank_s, 20))
    time_calls("q_core", lambda: qi.rolling_rank(close_rank_np, 20))

    return 0


if __name__ == "__main__":
    sys.exit(main())
