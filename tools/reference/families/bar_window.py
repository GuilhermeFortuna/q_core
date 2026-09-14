"""Bar-window reference family: StrategyEvaluator rolling window parity."""

from __future__ import annotations

import importlib.metadata
import platform
import sys
from pathlib import Path
from typing import Final, Literal

import numpy as np
import pandas as pd

from export_reference import (
    FORMAT,
    BackendSource,
    dumps_fixture,
    encode_column,
    git_blob_id,
    load_generator,
)

SCENARIO_IDS: Final = (
    "s01_seed",
    "s02_append",
    "s03_redeliver",
    "s04_overlap",
    "s05_late_full",
    "s06_gap_fill",
    "s07_unsorted_duplicates",
    "s08_empty",
    "s09_bound_one",
)

_OHLC: Final = ("open", "high", "low", "close")


class _StubStrategy:
    """Minimal strategy: pass-through indicators, no signals."""

    def __init__(self) -> None:
        from q_backend.backtesting.exit_strategy import ExitStrategy

        self.parameters: dict[str, object] = {}
        self.exit_strategy = ExitStrategy({})

    def compute_indicators(self, data: pd.DataFrame) -> pd.DataFrame:
        return data

    def check_entry_conditions(self, current_data: pd.Series) -> list[object]:
        return []

    def check_exit_conditions(
        self, current_data: pd.Series, open_trades: list[object]
    ) -> list[object]:
        return []

    def get_chart_indicators(self) -> list[object]:
        return []


def encode_time(index: pd.DatetimeIndex) -> dict[str, object]:
    """Encode bar open times as int64 microseconds; reject sub-microsecond remainders."""
    if not isinstance(index, pd.DatetimeIndex):
        raise TypeError(f"expected DatetimeIndex, got {type(index)!r}")
    naive = index.tz_convert(None) if index.tz is not None else index
    # Normalize to nanoseconds so both datetime64[us] (pandas 3) and [ns] work.
    ns = naive.as_unit("ns").asi8
    if np.any(ns % 1000 != 0):
        raise ValueError("timestamp has sub-microsecond remainder; cannot encode exactly as us")
    us = (ns // 1000).astype(np.int64)
    return encode_column(us)


def _ohlc(frame: pd.DataFrame) -> pd.DataFrame:
    return frame.loc[:, list(_OHLC)].copy()


def scenario_steps(
    bars: pd.DataFrame,
) -> dict[str, tuple[int, list[tuple[str, pd.DataFrame]]]]:
    """Map scenario id -> (bound, ordered steps of (operation, batch))."""
    ohlc = _ohlc(bars)

    def rows(*idxs: int) -> pd.DataFrame:
        return ohlc.iloc[list(idxs)]

    def slice_rows(start: int, stop: int) -> pd.DataFrame:
        return ohlc.iloc[start:stop]

    redeliver = slice_rows(30, 31).copy()
    redeliver.iloc[0, redeliver.columns.get_loc("close")] = float(redeliver.iloc[0]["close"]) + 1.0

    gap_seed = pd.concat([slice_rows(0, 10), slice_rows(12, 20)])

    dup_alt = rows(33).copy()
    for col in _OHLC:
        if col != "close":
            continue
        dup_alt.iloc[0, dup_alt.columns.get_loc(col)] = float(dup_alt.iloc[0][col]) + 1.0
    unsorted = pd.concat([rows(33), rows(31), dup_alt, rows(32)])

    empty = ohlc.iloc[0:0]

    return {
        "s01_seed": (20, [("seed_window", slice_rows(0, 30))]),
        "s02_append": (
            20,
            [
                ("seed_window", slice_rows(0, 30)),
                ("ingest_completed_bars", slice_rows(30, 31)),
            ],
        ),
        "s03_redeliver": (
            20,
            [
                ("seed_window", slice_rows(0, 30)),
                ("ingest_completed_bars", slice_rows(30, 31)),
                ("ingest_completed_bars", redeliver),
            ],
        ),
        "s04_overlap": (
            20,
            [
                ("seed_window", slice_rows(0, 30)),
                ("ingest_completed_bars", slice_rows(30, 31)),
                ("ingest_completed_bars", slice_rows(28, 33)),
            ],
        ),
        "s05_late_full": (
            20,
            [
                ("seed_window", slice_rows(0, 30)),
                ("ingest_completed_bars", slice_rows(30, 31)),
                ("ingest_completed_bars", slice_rows(5, 6)),
            ],
        ),
        "s06_gap_fill": (
            50,
            [
                ("seed_window", gap_seed),
                ("ingest_completed_bars", slice_rows(10, 12)),
            ],
        ),
        "s07_unsorted_duplicates": (
            20,
            [
                ("seed_window", slice_rows(0, 30)),
                ("ingest_completed_bars", unsorted),
            ],
        ),
        "s08_empty": (
            20,
            [
                ("seed_window", slice_rows(0, 30)),
                ("ingest_completed_bars", empty),
            ],
        ),
        "s09_bound_one": (
            1,
            [
                ("seed_window", slice_rows(0, 30)),
                ("ingest_completed_bars", slice_rows(30, 31)),
            ],
        ),
    }


def _encode_frame(frame: pd.DataFrame) -> dict[str, object]:
    payload: dict[str, object] = {"time": encode_time(frame.index)}
    for col in _OHLC:
        if col in frame.columns:
            payload[col] = encode_column(frame[col].to_numpy(dtype=np.float64))
        else:
            payload[col] = encode_column(np.asarray([], dtype=np.float64))
    return payload


def _make_evaluator(bound: int):
    from q_backend.execution.domain import StrategyIdentity
    from q_backend.execution.evaluator import StrategyEvaluator

    identity = StrategyIdentity(
        strategy_name="bar_window_stub",
        strategy_version=1,
        compiled_config={},
        config_hash="bar-window-ref",
        symbol="WIN$",
        timeframe="H1",
        sizing_config={"type": "fixed_quantity", "quantity": 1.0},
    )
    return StrategyEvaluator(
        deployment_id="bar-window-ref",
        identity=identity,
        strategy=_StubStrategy(),  # type: ignore[arg-type]
        window_bound=bound,
    )


class BarWindowFamily:
    """Drive StrategyEvaluator window updates and record each step."""

    name: Final[str] = "bar_window"
    environment: Final[Literal["numeric", "backend"]] = "backend"
    policy: Final[dict[str, object]] = {"kind": "exact"}

    def export(self, source: BackendSource, out_dir: Path) -> list[str]:
        generator = load_generator(source, "tests/backtesting/test_goldens.py", "synthetic_ohlcv")
        bars = generator(n=40)
        steps_by_id = scenario_steps(bars)

        sources = [
            {
                "path": "src/q_backend/execution/evaluator.py",
                "blob": git_blob_id(source, "src/q_backend/execution/evaluator.py"),
            },
            {
                "path": "src/q_backend/execution/bars.py",
                "blob": git_blob_id(source, "src/q_backend/execution/bars.py"),
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

        for scenario_id in SCENARIO_IDS:
            bound, steps = steps_by_id[scenario_id]
            evaluator = _make_evaluator(bound)
            cases: list[dict[str, object]] = []
            for step_i, (operation, batch) in enumerate(steps):
                if operation == "seed_window":
                    evaluator.seed_window(batch)
                elif operation == "ingest_completed_bars":
                    evaluator.ingest_completed_bars(batch)
                else:
                    raise ValueError(f"unknown operation {operation!r}")
                cases.append(
                    {
                        "case_id": f"{scenario_id}/step={step_i}",
                        "params": {"bound": bound, "tz": "UTC"},
                        "operation": operation,
                        "batch": _encode_frame(batch),
                        "expected": {"window": _encode_frame(evaluator._rolling)},
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
            sys.stderr.write(f"exported bar_window/{scenario_id}\n")

        return exported
