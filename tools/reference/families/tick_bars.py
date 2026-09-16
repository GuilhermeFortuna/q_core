"""Tick-bars reference family: resample, resolve_bar_ms, and indicator sampling."""

from __future__ import annotations

import importlib.metadata
import platform
import sys
from pathlib import Path
from typing import Any, Final, Literal, Sequence

import numpy as np

from export_reference import (
    FORMAT,
    BackendSource,
    dumps_fixture,
    encode_column,
    git_blob_id,
)

SCENARIO_IDS: Final = (
    "b01_last_price_m1",
    "b02_midpoint_when_no_last",
    "b03_nan_price_in_bar",
    "b04_pairwise_volume_lengths",
    "b05_interval_doubling",
    "b06_indicator_sampling_nan",
    "b07_out_of_order_ticks",
    "b08_synthetic_stream_m5",
)

PAIRWISE_LENGTHS: Final = (1, 7, 8, 9, 127, 128, 129, 1000, 20_000)
M1_MS: Final = 60_000
M5_MS: Final = 300_000
H12_MS: Final = 43_200_000
DAY_MS: Final = 86_400_000


def encode_indicator_samples(samples: Sequence[float | None]) -> dict[str, object]:
    """Encode sampler output; Python ``None`` (missing) becomes NaN in the column."""
    values = np.asarray(
        [float("nan") if s is None else float(s) for s in samples],
        dtype=np.float64,
    )
    return encode_column(values)


def encode_tick_bars(
    open_msc: np.ndarray,
    open_: np.ndarray,
    high: np.ndarray,
    low: np.ndarray,
    close: np.ndarray,
    volume: np.ndarray,
    tick_start: np.ndarray,
    tick_end: np.ndarray,
) -> dict[str, object]:
    return {
        "open_msc": encode_column(np.asarray(open_msc, dtype=np.int64)),
        "open": encode_column(np.asarray(open_, dtype=np.float64)),
        "high": encode_column(np.asarray(high, dtype=np.float64)),
        "low": encode_column(np.asarray(low, dtype=np.float64)),
        "close": encode_column(np.asarray(close, dtype=np.float64)),
        "volume": encode_column(np.asarray(volume, dtype=np.int64)),
        "tick_start": encode_column(np.asarray(tick_start, dtype=np.int64)),
        "tick_end": encode_column(np.asarray(tick_end, dtype=np.int64)),
    }


def scenario_tick_inputs() -> dict[str, dict[str, Any]]:
    """Scenario builders used by unit tests and export (no backend import)."""
    return {
        "b01_last_price_m1": _b01_last_price_m1(),
        "b02_midpoint_when_no_last": _b02_midpoint_when_no_last(),
        "b03_nan_price_in_bar": _b03_nan_price_in_bar(),
        "b04_pairwise_volume_lengths": _b04_pairwise_volume_lengths(),
        "b05_interval_doubling": _b05_interval_doubling(),
        "b06_indicator_sampling_nan": _b06_indicator_sampling_nan(),
        "b07_out_of_order_ticks": _b07_out_of_order_ticks(),
        "b08_synthetic_stream_m5": _b08_synthetic_stream_m5(),
    }


def _arrays(
    time_msc: Sequence[int] | np.ndarray,
    bid: Sequence[float] | np.ndarray,
    ask: Sequence[float] | np.ndarray,
    last: Sequence[float] | np.ndarray,
    volume: Sequence[float] | np.ndarray,
    *,
    bar_ms: int,
) -> dict[str, Any]:
    return {
        "time_msc": np.asarray(time_msc, dtype=np.int64),
        "bid": np.asarray(bid, dtype=np.float64),
        "ask": np.asarray(ask, dtype=np.float64),
        "last": np.asarray(last, dtype=np.float64),
        "volume": np.asarray(volume, dtype=np.float64),
        "bar_ms": int(bar_ms),
    }


def _b01_last_price_m1() -> dict[str, Any]:
    # One positive last forces last-price OHLC for the whole stream.
    return _arrays(
        [0, 1_000, 60_000, 61_000],
        [100.0, 101.0, 102.0, 103.0],
        [100.2, 101.2, 102.2, 103.2],
        [100.1, 101.1, 102.1, 103.1],
        [1.0, 1.0, 1.0, 1.0],
        bar_ms=M1_MS,
    )


def _b02_midpoint_when_no_last() -> dict[str, Any]:
    return _arrays(
        [0, 1_000, 60_000, 61_000],
        [100.0, 101.0, 102.0, 103.0],
        [100.2, 101.2, 102.2, 103.2],
        [0.0, 0.0, 0.0, 0.0],
        [1.0, 1.0, 1.0, 1.0],
        bar_ms=M1_MS,
    )


def _b03_nan_price_in_bar() -> dict[str, Any]:
    return _arrays(
        [0, 1_000, 2_000],
        [100.0, float("nan"), 102.0],
        [100.2, float("nan"), 102.2],
        [0.0, 0.0, 0.0],
        [1.0, 1.0, 1.0],
        bar_ms=M1_MS,
    )


def _b04_pairwise_volume_lengths() -> dict[str, Any]:
    """One bar per pairwise length so the gate, not a unit test, owns the sums."""
    rng = np.random.default_rng(0)
    time_parts: list[np.ndarray] = []
    vol_parts: list[np.ndarray] = []
    bid_parts: list[np.ndarray] = []
    for bar_i, length in enumerate(PAIRWISE_LENGTHS):
        # Sequential rng.random(n) draws match the recorded numpy_sum fixtures.
        vols = rng.random(length).astype(np.float64)
        t0 = bar_i * M1_MS
        times = t0 + np.arange(length, dtype=np.int64)
        time_parts.append(times)
        vol_parts.append(vols)
        bid_parts.append(np.full(length, 100.0 + bar_i, dtype=np.float64))
    time_msc = np.concatenate(time_parts)
    volume = np.concatenate(vol_parts)
    bid = np.concatenate(bid_parts)
    ask = bid + 0.1
    last = np.zeros_like(bid)
    return _arrays(time_msc, bid, ask, last, volume, bar_ms=M1_MS)


def _b05_interval_doubling() -> dict[str, Any]:
    # Sparse ticks spanning 40 days; resolve M1 → 120_000.
    span_40d = 3_456_000_000
    sparse_times = np.array([0, span_40d], dtype=np.int64)
    return {
        "resolve_cases": [
            {
                "case_id": "m1_forty_days",
                "display_timeframe": "M1",
                "base_bar_ms": M1_MS,
                "span_msc": span_40d,
            },
            {
                "case_id": "h12_past_d1",
                "display_timeframe": "H12",
                "base_bar_ms": H12_MS,
                "span_msc": 5_000_000_000_000,
            },
            {
                "case_id": "m1_zero_span",
                "display_timeframe": "M1",
                "base_bar_ms": M1_MS,
                "span_msc": 0,
            },
        ],
        "sparse_ticks": _arrays(
            sparse_times,
            [100.0, 101.0],
            [100.2, 101.2],
            [100.1, 101.1],
            [1.0, 1.0],
            bar_ms=M1_MS,  # exporter resolves then resamples
        ),
    }


def _b06_indicator_sampling_nan() -> dict[str, Any]:
    ticks = _arrays(
        [0, 1_000, 60_000, 61_000],
        [100.0, 101.0, 102.0, 103.0],
        [100.2, 101.2, 102.2, 103.2],
        [100.1, 101.1, 102.1, 103.1],
        [1.0, 1.0, 1.0, 1.0],
        bar_ms=M1_MS,
    )
    # Series aligned to ticks; NaN at the last tick of the first bar → None → NaN.
    series = np.array([1.0, float("nan"), 3.0, 4.0], dtype=np.float64)
    return {**ticks, "series": series}


def _b07_out_of_order_ticks() -> dict[str, Any]:
    # Buckets 0, 1, 0 → three bars with repeated open_msc=0.
    return _arrays(
        [0, 70_000, 30_000],
        [100.0, 101.0, 102.0],
        [100.2, 101.2, 102.2],
        [100.1, 101.1, 102.1],
        [1.0, 1.0, 1.0],
        bar_ms=M1_MS,
    )


def _b08_synthetic_stream_m5() -> dict[str, Any]:
    n = 2_000
    seed = 20240609
    base_msc = 1_700_000_000_000
    msc_per_tick = 40_000
    rng = np.random.default_rng(seed)
    trend = 100.0 + 8.0 * np.sin(np.linspace(0.0, 6.0 * np.pi, n))
    noise = np.cumsum(rng.normal(0.0, 0.02, size=n))
    last = (trend + noise).astype(np.float64)
    spread = 0.02
    time_msc = base_msc + np.arange(n, dtype=np.int64) * msc_per_tick
    return _arrays(
        time_msc,
        (last - spread / 2).astype(np.float64),
        (last + spread / 2).astype(np.float64),
        last,
        np.ones(n, dtype=np.float64),
        bar_ms=M5_MS,
    )


def _bars_from_resample(ticks: Any, bar_ms: int) -> dict[str, object]:
    from q_backend.backtesting.tick.chart_data import _resample_ticks_to_bars

    bars, bar_ranges = _resample_ticks_to_bars(ticks, bar_ms)
    n_bars = len(bars)
    open_msc = np.empty(n_bars, dtype=np.int64)
    open_ = np.empty(n_bars, dtype=np.float64)
    high = np.empty(n_bars, dtype=np.float64)
    low = np.empty(n_bars, dtype=np.float64)
    close = np.empty(n_bars, dtype=np.float64)
    volume = np.empty(n_bars, dtype=np.int64)
    tick_start = np.empty(n_bars, dtype=np.int64)
    tick_end = np.empty(n_bars, dtype=np.int64)
    for i, (bar, (start, end)) in enumerate(zip(bars, bar_ranges, strict=True)):
        open_msc[i] = int((int(ticks.time_msc[start]) // bar_ms) * bar_ms)
        open_[i] = float(bar["open"])
        high[i] = float(bar["high"])
        low[i] = float(bar["low"])
        close[i] = float(bar["close"])
        volume[i] = int(bar["volume"])
        tick_start[i] = int(start)
        tick_end[i] = int(end)
    return encode_tick_bars(open_msc, open_, high, low, close, volume, tick_start, tick_end)


def _to_tick_arrays(inputs: dict[str, Any]) -> Any:
    from q_backend.backtesting.tick.strategy import TickArrays

    return TickArrays(
        time_msc=np.asarray(inputs["time_msc"], dtype=np.int64),
        bid=np.asarray(inputs["bid"], dtype=np.float64),
        ask=np.asarray(inputs["ask"], dtype=np.float64),
        last=np.asarray(inputs["last"], dtype=np.float64),
        volume=np.asarray(inputs["volume"], dtype=np.float64),
    )


def _encode_tick_inputs(inputs: dict[str, Any], *, bar_ms: int | None = None) -> dict[str, object]:
    encoded: dict[str, object] = {
        "time_msc": encode_column(np.asarray(inputs["time_msc"], dtype=np.int64)),
        "bid": encode_column(np.asarray(inputs["bid"], dtype=np.float64)),
        "ask": encode_column(np.asarray(inputs["ask"], dtype=np.float64)),
        "last": encode_column(np.asarray(inputs["last"], dtype=np.float64)),
        "volume": encode_column(np.asarray(inputs["volume"], dtype=np.float64)),
    }
    ms = int(inputs["bar_ms"] if bar_ms is None else bar_ms)
    encoded["bar_ms"] = ms
    return encoded


def _assert_b04_lengths(expected: dict[str, object]) -> None:
    starts = expected["tick_start"]["values"]  # type: ignore[index]
    ends = expected["tick_end"]["values"]  # type: ignore[index]
    assert isinstance(starts, list) and isinstance(ends, list)
    lengths = [int(e) - int(s) for s, e in zip(starts, ends, strict=True)]
    if lengths != list(PAIRWISE_LENGTHS):
        raise RuntimeError(
            f"b04_pairwise_volume_lengths: expected bar lengths {list(PAIRWISE_LENGTHS)}, got {lengths}"
        )


def _assert_b07_repeated_bucket(expected: dict[str, object]) -> None:
    open_msc = expected["open_msc"]["values"]  # type: ignore[index]
    assert isinstance(open_msc, list)
    if len(open_msc) == len(set(int(v) for v in open_msc)):
        raise RuntimeError(
            f"b07_out_of_order_ticks: expected a repeated bucket, got open_msc={open_msc}"
        )


class TickBarsFamily:
    """Drive chart resample / resolve / sample helpers for tick_bars fixtures."""

    name: Final[str] = "tick_bars"
    environment: Final[Literal["numeric", "backend"]] = "backend"
    policy: Final[dict[str, object]] = {"kind": "exact"}

    def export(self, source: BackendSource, out_dir: Path) -> list[str]:
        from q_backend.backtesting.tick.chart_data import (
            _resolve_bar_ms,
            _sample_indicator_at_bars,
        )

        sources = [
            {
                "path": "src/q_backend/backtesting/tick/chart_data.py",
                "blob": git_blob_id(source, "src/q_backend/backtesting/tick/chart_data.py"),
            },
            {
                "path": "tests/backtesting/test_goldens.py",
                "blob": git_blob_id(source, "tests/backtesting/test_goldens.py"),
            },
        ]
        provenance = {
            "backend_repo": source.repo_url,
            "backend_rev": source.rev,
            "exporter": "tools/reference/export_reference.py",
            "environment": "backend",
            "python": platform.python_version(),
            "numpy": importlib.metadata.version("numpy"),
            "pandas": importlib.metadata.version("pandas"),
            "sources": sources,
        }

        family_dir = out_dir / self.name
        family_dir.mkdir(parents=True, exist_ok=True)
        exported: list[str] = []
        scenarios = scenario_tick_inputs()

        for scenario_id in SCENARIO_IDS:
            raw = scenarios[scenario_id]
            cases: list[dict[str, object]] = []

            if scenario_id == "b05_interval_doubling":
                for resolve_case in raw["resolve_cases"]:
                    display = str(resolve_case["display_timeframe"])
                    span_msc = int(resolve_case["span_msc"])
                    base_bar_ms = int(resolve_case["base_bar_ms"])
                    resolved = int(_resolve_bar_ms(display, span_msc))
                    cases.append(
                        {
                            "case_id": f"{scenario_id}/{resolve_case['case_id']}",
                            "operation": "resolve_bar_ms",
                            "params": {
                                "display_timeframe": display,
                                "base_bar_ms": base_bar_ms,
                                "span_msc": span_msc,
                            },
                            "inputs": {
                                "base_bar_ms": base_bar_ms,
                                "span_msc": span_msc,
                            },
                            "expected": {"bar_ms": resolved},
                        }
                    )
                # Also pin sparse M1 resampling at the resolved interval.
                sparse = raw["sparse_ticks"]
                span = int(sparse["time_msc"][-1] - sparse["time_msc"][0])
                resolved_ms = int(_resolve_bar_ms("M1", span))
                ticks = _to_tick_arrays(sparse)
                expected = _bars_from_resample(ticks, resolved_ms)
                cases.append(
                    {
                        "case_id": f"{scenario_id}/sparse_m1_bars",
                        "operation": "tick_bars",
                        "params": {"display_timeframe": "M1", "resolved_bar_ms": resolved_ms},
                        "inputs": _encode_tick_inputs(sparse, bar_ms=resolved_ms),
                        "expected": expected,
                    }
                )
                if cases[0]["expected"]["bar_ms"] != 120_000:  # type: ignore[index]
                    raise RuntimeError(
                        f"{scenario_id}: M1/40d resolve expected 120000, "
                        f"got {cases[0]['expected']['bar_ms']}"  # type: ignore[index]
                    )
                if cases[1]["expected"]["bar_ms"] != 172_800_000:  # type: ignore[index]
                    raise RuntimeError(
                        f"{scenario_id}: H12 resolve expected 172800000, "
                        f"got {cases[1]['expected']['bar_ms']}"  # type: ignore[index]
                    )
            elif scenario_id == "b06_indicator_sampling_nan":
                ticks = _to_tick_arrays(raw)
                bar_ms = int(raw["bar_ms"])
                bars_expected = _bars_from_resample(ticks, bar_ms)
                # Rebuild ranges for the sampler from encoded starts/ends.
                starts = bars_expected["tick_start"]["values"]  # type: ignore[index]
                ends = bars_expected["tick_end"]["values"]  # type: ignore[index]
                assert isinstance(starts, list) and isinstance(ends, list)
                bar_ranges = [(int(s), int(e)) for s, e in zip(starts, ends, strict=True)]
                samples = _sample_indicator_at_bars(
                    np.asarray(raw["series"], dtype=np.float64), bar_ranges
                )
                if None not in samples:
                    raise RuntimeError(f"{scenario_id}: expected a None sample from NaN series")
                cases.append(
                    {
                        "case_id": f"{scenario_id}/bars",
                        "operation": "tick_bars",
                        "params": {},
                        "inputs": _encode_tick_inputs(raw),
                        "expected": bars_expected,
                    }
                )
                cases.append(
                    {
                        "case_id": f"{scenario_id}/sample_at_bar_ends",
                        "operation": "sample_at_bar_ends",
                        "params": {},
                        "inputs": {
                            "series": encode_column(np.asarray(raw["series"], dtype=np.float64)),
                            "tick_end": encode_column(
                                np.asarray(ends, dtype=np.int64)
                            ),
                        },
                        "expected": {"values": encode_indicator_samples(samples)},
                    }
                )
            else:
                ticks = _to_tick_arrays(raw)
                bar_ms = int(raw["bar_ms"])
                expected = _bars_from_resample(ticks, bar_ms)
                if scenario_id == "b04_pairwise_volume_lengths":
                    _assert_b04_lengths(expected)
                if scenario_id == "b07_out_of_order_ticks":
                    _assert_b07_repeated_bucket(expected)
                cases.append(
                    {
                        "case_id": scenario_id,
                        "operation": "tick_bars",
                        "params": {},
                        "inputs": _encode_tick_inputs(raw),
                        "expected": expected,
                    }
                )

            payload = {
                "format": FORMAT,
                "family": self.name,
                "fixture_id": scenario_id,
                "policy": dict(self.policy),
                "provenance": provenance,
                "cases": cases,
            }
            path = family_dir / f"{scenario_id}.json"
            path.write_text(dumps_fixture(payload), encoding="utf-8")
            exported.append(scenario_id)
            sys.stderr.write(f"exported tick_bars/{scenario_id}\n")

        return exported
