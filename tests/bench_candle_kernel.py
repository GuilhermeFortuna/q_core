"""Compare candle kernel throughput vs the pinned Python backtest engine.

Five runs over a 50_000-bar synthetic series with scripted decisions and a trailing stop.
Not part of make check.
"""

from __future__ import annotations

import os
import shutil
import statistics
import subprocess
import sys
import tempfile
import time
from pathlib import Path

import numpy as np

ROOT = Path(__file__).parents[1]
BACKEND_REV = (ROOT / "BACKEND_REV").read_text(encoding="utf-8").strip()
BACKEND_REPO = os.environ.get(
    "Q_BACKEND_REPO",
    "https://github.com/GuilhermeFortuna/q_backend.git",
)
BACKEND_CHECKOUT = os.environ.get("Q_BACKEND_CHECKOUT")

RUNS = 5
BARS = 50_000
H = 3_600_000_000

ENTRY = "entry_signal"
EXIT_LONG = "exit_long_signal"
EXIT_SHORT = "exit_short_signal"
STRENGTH = "signal_strength"


def _backend_dir() -> tuple[Path, bool]:
    if BACKEND_CHECKOUT:
        return Path(BACKEND_CHECKOUT), False
    tmp = Path(tempfile.mkdtemp(prefix="q027-bench-backend-"))
    checkout = tmp / "q_backend"
    subprocess.check_call(["git", "clone", "--quiet", BACKEND_REPO, str(checkout)])
    subprocess.check_call(["git", "-C", str(checkout), "checkout", "--quiet", BACKEND_REV])
    subprocess.check_call(["uv", "sync", "--frozen"], cwd=str(checkout))
    return checkout, True


def _scripted_decisions(n: int, seed: int = 27_100):
    rng = np.random.default_rng(seed)
    buy = rng.random(n) < 0.08
    sell = rng.random(n) < 0.08
    exit_long = rng.random(n) < 0.12
    exit_short = rng.random(n) < 0.12
    entry = np.zeros(n, dtype=np.int8)
    entry[sell] = -1
    entry[buy] = 1
    strength = np.zeros(n, dtype=np.float64)
    choices = np.array([0.25, 0.5, 0.75, 1.0])
    drawn = rng.choice(choices, size=n)
    strength[entry != 0] = drawn[entry != 0]
    return entry, exit_long, exit_short, strength


def _synthetic_ohlcv(n: int, seed: int = 20240609):
    rng = np.random.default_rng(seed)
    times = (np.arange(n) * H).astype("datetime64[us]")
    close = 90.0 + np.cumsum(rng.normal(0.0, 0.5, n))
    open_ = np.concatenate([[close[0]], close[:-1]])
    spread = rng.uniform(0.2, 1.0, n)
    high = np.maximum(open_, close) + spread
    low = np.minimum(open_, close) - spread
    return times, open_, high, low, close


def _atr_14(high: np.ndarray, low: np.ndarray, close: np.ndarray, period: int = 14) -> np.ndarray:
    prev = np.concatenate([[close[0]], close[:-1]])
    tr = np.maximum(high - low, np.maximum(np.abs(high - prev), np.abs(low - prev)))
    out = np.full(len(close), np.nan)
    for i in range(period - 1, len(close)):
        out[i] = tr[i - period + 1 : i + 1].mean()
    return out


def build_series() -> dict[str, np.ndarray]:
    times, open_, high, low, close = _synthetic_ohlcv(BARS)
    entry, exit_long, exit_short, strength = _scripted_decisions(BARS)
    time_us = times.astype("datetime64[us]").view("int64")
    atr = _atr_14(high, low, close)
    return {
        "time_us": time_us,
        "open": open_.astype(np.float64),
        "high": high.astype(np.float64),
        "low": low.astype(np.float64),
        "close": close.astype(np.float64),
        ENTRY: entry,
        EXIT_LONG: exit_long.astype(bool),
        EXIT_SHORT: exit_short.astype(bool),
        STRENGTH: strength,
        "atr_14": atr.astype(np.float64),
    }


def _kernel_kwargs(series: dict[str, np.ndarray]) -> dict:
    return dict(
        time_us=series["time_us"],
        open=series["open"],
        high=series["high"],
        low=series["low"],
        close=series["close"],
        entry=series[ENTRY],
        exit_long=series[EXIT_LONG],
        exit_short=series[EXIT_SHORT],
        strength=series[STRENGTH],
        columns={"atr_14": series["atr_14"]},
        initial_capital=100_000.0,
        point_value=1.0,
        sizing={
            "type": "fixed_quantity",
            "quantity": 1.0,
            "scale_by_signal_strength": False,
        },
        sizing_point_value=1.0,
        exit_params={"trailing_stop_pct": 0.03, "atr_period": 14, "stop_loss_atr": 2.0},
        force_close_at_end=False,
    )


def _python_frame(series: dict[str, np.ndarray], backend: Path):
    sys.path.insert(0, str(ROOT / "tools" / "reference"))
    sys.path.insert(0, str(backend / "src"))
    import pandas as pd
    from families.scripted_strategy import Decisions, augment, build_scripted_strategy

    index = pd.to_datetime(series["time_us"], unit="us")
    frame = pd.DataFrame(
        {
            "open": series["open"],
            "high": series["high"],
            "low": series["low"],
            "close": series["close"],
            "atr_14": series["atr_14"],
        },
        index=index,
    )
    decisions = Decisions(
        series[ENTRY],
        series[EXIT_LONG],
        series[EXIT_SHORT],
        series[STRENGTH],
    )
    strategy = build_scripted_strategy(
        decisions=decisions,
        trailing_stop_pct=0.03,
        atr_period=14,
        stop_loss_atr=2.0,
    )
    return augment(strategy, frame), strategy


def _python_engine(frame, strategy, backend: Path):
    sys.path.insert(0, str(backend / "src"))
    from q_backend.backtesting.engine import BacktestEngine
    from q_backend.backtesting.position_sizing import build_position_sizer
    from families.scripted_strategy import SYMBOL

    sizer = build_position_sizer(
        {"type": "fixed_quantity", "quantity": 1.0, "scale_by_signal_strength": False},
        point_value=1.0,
    )
    return BacktestEngine(
        strategy=strategy,
        sizer=sizer,
        initial_capital=100_000.0,
        point_values={SYMBOL: 1.0},
    )


def _ledger_summary(result: dict) -> tuple[int, float, float]:
    pnl = np.asarray(result["pnl"], dtype=np.float64)
    commission = np.asarray(result["commission"], dtype=np.float64)
    return len(pnl), float(np.nansum(pnl)), float(np.sum(commission))


def _python_summary(registry) -> tuple[int, float, float]:
    trades = registry.get_all_trades()
    pnl = np.array([float("nan") if t.pnl is None else float(t.pnl) for t in trades])
    commission = np.array([float(t.commission) for t in trades])
    return len(trades), float(np.nansum(pnl)), float(np.sum(commission))


def bench_kernel(series: dict[str, np.ndarray]) -> list[float]:
    import q_core.engine as engine

    kwargs = _kernel_kwargs(series)
    engine.run_candle(**kwargs)
    times = []
    for _ in range(RUNS):
        t0 = time.perf_counter()
        engine.run_candle(**kwargs)
        times.append(time.perf_counter() - t0)
    return times


def bench_python(series: dict[str, np.ndarray], backend: Path) -> list[float]:
    frame, strategy = _python_frame(series, backend)
    engine = _python_engine(frame, strategy, backend)
    engine._run_single_chunk(frame, force_close_at_end=False)
    times = []
    for _ in range(RUNS):
        t0 = time.perf_counter()
        engine._run_single_chunk(frame, force_close_at_end=False)
        times.append(time.perf_counter() - t0)
    return times


def main() -> None:
    series = build_series()
    backend, cleanup = _backend_dir()
    try:
        import q_core.engine as engine

        frame, strategy = _python_frame(series, backend)
        py_engine = _python_engine(frame, strategy, backend)
        registry = py_engine._run_single_chunk(frame, force_close_at_end=False)
        kernel = engine.run_candle(**_kernel_kwargs(series))
        assert _python_summary(registry) == _ledger_summary(kernel)

        py_times = bench_python(series, backend)
        kernel_times = bench_kernel(series)

        def report(label: str, samples: list[float]) -> None:
            bps = BARS / np.array(samples)
            print(f"{label} wall s:", ", ".join(f"{t:.3f}" for t in samples))
            print(f"{label} median wall s:", f"{statistics.median(samples):.3f}")
            print(f"{label} bars/s:", ", ".join(f"{v:.0f}" for v in bps))
            print(f"{label} median bars/s:", f"{statistics.median(bps):.0f}")

        report("Python engine", py_times)
        report("q_core.engine", kernel_times)
    finally:
        if cleanup:
            shutil.rmtree(backend.parent, ignore_errors=True)


if __name__ == "__main__":
    main()
