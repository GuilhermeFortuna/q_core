"""Benchmark numba tick simulate vs q_core.engine.tick_simulate.

Times the pinned backend kernel on a synthetic stream (default 5_000_000 ticks):
one cold run in a fresh process with an empty numba cache, five warm numba runs,
and five rust runs. Asserts equal ledgers before reporting wall times.
Not part of make check.
"""

from __future__ import annotations

import os
import statistics
import subprocess
import sys
import tempfile
import time
from typing import Any

import numpy as np

N_TICKS = int(os.environ.get("N_TICKS", "5_000_000"))
WARM_RUNS = 5
INITIAL_CAPITAL = 100_000.0
POINT_VALUE = 1.0
SIZING_MODE = 0
SIZING_A = 1.0
SIZING_B = 0.0
SIZING_C = 0.0


def _make_stream(n: int) -> dict[str, np.ndarray]:
    from q_backend.backtesting.tick.strategies.tick_ma_breakout import TickMaBreakoutStrategy
    from q_backend.backtesting.tick.strategy import TickArrays

    seed = 20240609
    base_msc = 1_700_000_000_000
    msc_per_tick = 40_000
    rng = np.random.default_rng(seed)
    trend = 100.0 + 8.0 * np.sin(np.linspace(0.0, 6.0 * np.pi, n))
    noise = np.cumsum(rng.normal(0.0, 0.02, size=n))
    last = (trend + noise).astype(np.float64)
    spread = 0.02
    time_msc = base_msc + np.arange(n, dtype=np.int64) * msc_per_tick
    ticks = TickArrays(
        time_msc=time_msc,
        bid=(last - spread / 2).astype(np.float64),
        ask=(last + spread / 2).astype(np.float64),
        last=last,
        volume=np.ones(n, dtype=np.float64),
    )
    strategy = TickMaBreakoutStrategy(
        short_period=20,
        long_period=50,
        threshold=0.0,
        sl_points=0.3,
        tp_points=0.6,
        symbol="SYNTH",
    )
    signals = strategy.compute_signals(ticks)
    return {
        "bid": ticks.bid,
        "ask": ticks.ask,
        "direction": signals.direction.astype(np.int8),
        "sl_points": signals.sl_points.astype(np.float64),
        "tp_points": signals.tp_points.astype(np.float64),
    }


def _run_numba(stream: dict[str, np.ndarray]) -> tuple[Any, ...]:
    from q_backend.backtesting.tick.kernel import simulate

    return simulate(
        stream["bid"],
        stream["ask"],
        stream["direction"],
        stream["sl_points"],
        stream["tp_points"],
        INITIAL_CAPITAL,
        POINT_VALUE,
        SIZING_MODE,
        SIZING_A,
        SIZING_B,
        SIZING_C,
    )


def _run_rust(stream: dict[str, np.ndarray]) -> dict[str, Any]:
    import q_core.engine as eng

    return eng.tick_simulate(
        bid=stream["bid"],
        ask=stream["ask"],
        direction=stream["direction"],
        sl_points=stream["sl_points"],
        tp_points=stream["tp_points"],
        initial_capital=INITIAL_CAPITAL,
        point_value=POINT_VALUE,
        sizing={"type": "fixed_quantity", "quantity": SIZING_A},
    )


def _ledger_from_numba(result: tuple[Any, ...]) -> dict[str, Any]:
    trade_count = int(result[7])
    return {
        "entry_idx": np.asarray(result[0][:trade_count], dtype=np.int64),
        "exit_idx": np.asarray(result[1][:trade_count], dtype=np.int64),
        "entry_price": np.asarray(result[2][:trade_count], dtype=np.float64),
        "exit_price": np.asarray(result[3][:trade_count], dtype=np.float64),
        "direction": np.asarray(result[4][:trade_count], dtype=np.int64),
        "quantity": np.asarray(result[5][:trade_count], dtype=np.float64),
        "exit_reason": np.asarray(result[6][:trade_count], dtype=np.int64),
        "final_capital": float(result[8]),
    }


def _ledger_from_rust(result: dict[str, Any]) -> dict[str, Any]:
    tc = len(result["entry_idx"])
    return {
        "entry_idx": np.asarray(result["entry_idx"], dtype=np.int64),
        "exit_idx": np.asarray(result["exit_idx"], dtype=np.int64),
        "entry_price": np.asarray(result["entry_price"], dtype=np.float64),
        "exit_price": np.asarray(result["exit_price"], dtype=np.float64),
        "direction": np.asarray(result["direction"], dtype=np.int64),
        "quantity": np.asarray(result["quantity"], dtype=np.float64),
        "exit_reason": np.asarray(result["exit_reason"], dtype=np.int64),
        "final_capital": float(result["final_capital"]),
    }


def _assert_ledgers_equal(numba_raw: tuple[Any, ...], rust_raw: dict[str, Any]) -> dict[str, Any]:
    numba = _ledger_from_numba(numba_raw)
    rust = _ledger_from_rust(rust_raw)
    for key in (
        "entry_idx",
        "exit_idx",
        "entry_price",
        "exit_price",
        "direction",
        "quantity",
        "exit_reason",
    ):
        if key in ("entry_price", "exit_price", "quantity"):
            assert np.array_equal(numba[key], rust[key], equal_nan=True), f"{key} mismatch"
        else:
            assert np.array_equal(numba[key], rust[key]), f"{key} mismatch"
    assert numba["final_capital"] == rust["final_capital"], "final_capital mismatch"
    return numba


def _bench_numba_cold() -> float:
    cache_dir = tempfile.mkdtemp(prefix="numba-cache-")
    env = os.environ.copy()
    env["NUMBA_CACHE_DIR"] = cache_dir
    env["N_TICKS"] = str(N_TICKS)
    proc = subprocess.run(
        [sys.executable, __file__, "--cold"],
        env=env,
        capture_output=True,
        text=True,
        check=True,
    )
    return float(proc.stdout.strip())


def _cold_main() -> None:
    stream = _make_stream(N_TICKS)
    t0 = time.perf_counter()
    _run_numba(stream)
    print(f"{time.perf_counter() - t0:.6f}")


def _fmt_times(times: list[float]) -> str:
    return ", ".join(f"{t:.3f}" for t in times)


def main() -> None:
    print(f"Building synthetic stream ({N_TICKS:,} ticks)...")
    stream = _make_stream(N_TICKS)

    print("Checking ledger parity (numba vs rust)...")
    numba_result = _run_numba(stream)
    rust_result = _run_rust(stream)
    ledger = _assert_ledgers_equal(numba_result, rust_result)
    trade_count = len(ledger["entry_idx"])
    print(f"Parity ok ({trade_count:,} trades, final_capital={ledger['final_capital']:.2f})")

    print("Timing numba cold run (fresh process, empty cache)...")
    cold = _bench_numba_cold()

    warm_numba: list[float] = []
    for _ in range(WARM_RUNS):
        t0 = time.perf_counter()
        _run_numba(stream)
        warm_numba.append(time.perf_counter() - t0)

    warm_rust: list[float] = []
    for _ in range(WARM_RUNS):
        t0 = time.perf_counter()
        _run_rust(stream)
        warm_rust.append(time.perf_counter() - t0)

    print(f"N_TICKS: {N_TICKS:,}")
    print(f"trades: {trade_count:,}")
    print(f"numba cold wall s (compile+run): {cold:.3f}")
    print(f"numba warm wall s: {_fmt_times(warm_numba)}")
    print(f"numba warm median s: {statistics.median(warm_numba):.3f}")
    print(f"rust warm wall s: {_fmt_times(warm_rust)}")
    print(f"rust warm median s: {statistics.median(warm_rust):.3f}")
    print(
        f"warm throughput (median ticks/s): "
        f"numba {N_TICKS / statistics.median(warm_numba):,.0f}, "
        f"rust {N_TICKS / statistics.median(warm_rust):,.0f}"
    )


if __name__ == "__main__":
    if len(sys.argv) > 1 and sys.argv[1] == "--cold":
        _cold_main()
    else:
        main()
