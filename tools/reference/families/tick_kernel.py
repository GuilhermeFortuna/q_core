"""Tick-kernel reference family: simulate and day-split parity."""

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
    "t01_sl_before_tp",
    "t02_tp_only",
    "t03_opposite_signal_no_reentry",
    "t04_long_spread_loss",
    "t05_short_spread_loss",
    "t06_end_of_stream_close",
    "t07_nan_levels",
    "t08_fractional_fixed_quantity",
    "t09_safety_margin_min_max",
    "t10_negative_capital",
    "t11_synthetic_stream",
    "t12_day_bounds",
)

DAY_MS: Final = 86_400_000


def encode_ledger(result: tuple[Any, ...]) -> dict[str, object]:
    """Slice numba's length-n ledger arrays to ``trade_count`` and encode columns."""
    (
        entry_idx,
        exit_idx,
        entry_price,
        exit_price,
        direction,
        quantity,
        exit_reason,
        trade_count,
        final_capital,
    ) = result
    tc = int(trade_count)
    return {
        "entry_idx": encode_column(np.asarray(entry_idx[:tc], dtype=np.int64)),
        "exit_idx": encode_column(np.asarray(exit_idx[:tc], dtype=np.int64)),
        "entry_price": encode_column(np.asarray(entry_price[:tc], dtype=np.float64)),
        "exit_price": encode_column(np.asarray(exit_price[:tc], dtype=np.float64)),
        "direction": encode_column(np.asarray(direction[:tc], dtype=np.int64)),
        "quantity": encode_column(np.asarray(quantity[:tc], dtype=np.float64)),
        "exit_reason": encode_column(np.asarray(exit_reason[:tc], dtype=np.int64)),
        "final_capital": float(final_capital),
    }


def encode_day_bounds(starts: Sequence[int], ends: Sequence[int]) -> dict[str, object]:
    return {
        "starts": encode_column(np.asarray(list(starts), dtype=np.int64)),
        "ends": encode_column(np.asarray(list(ends), dtype=np.int64)),
    }


def day_bounds_from_chunks(chunks: Sequence[Any]) -> tuple[list[int], list[int]]:
    """Derive half-open ``[starts, ends)`` from ``_split_ticks_by_day`` chunk lengths."""
    starts: list[int] = []
    ends: list[int] = []
    offset = 0
    for chunk in chunks:
        n = len(chunk.time_msc)
        starts.append(offset)
        offset += n
        ends.append(offset)
    return starts, ends


def _fixed_quantity(quantity: float) -> dict[str, object]:
    return {
        "type": "fixed_quantity",
        "quantity": float(quantity),
        "scale_by_signal_strength": False,
    }


def _safety_margin(
    margin: float,
    *,
    min_contracts: int = 1,
    max_contracts: int | None = None,
) -> dict[str, object]:
    return {
        "type": "fixed_safety_margin",
        "safety_margin_per_contract": float(margin),
        "min_contracts": int(min_contracts),
        "max_contracts": max_contracts,
        "scale_by_signal_strength": False,
    }


def _simulate_scenarios() -> dict[str, dict[str, Any]]:
    """Hand-built simulate scenarios (t01–t10) matching decisions / test_kernel shapes."""
    nan = float("nan")
    return {
        "t01_sl_before_tp": {
            "bid": np.array([100.0, 97.0, 104.0], dtype=np.float64),
            "ask": np.array([100.0, 97.0, 104.0], dtype=np.float64),
            "direction": np.array([1, 0, 0], dtype=np.int8),
            "sl_points": np.array([2.0, 2.0, 2.0], dtype=np.float64),
            "tp_points": np.array([3.0, 3.0, 3.0], dtype=np.float64),
            "initial_capital": 100_000.0,
            "point_value": 1.0,
            "sizing": _fixed_quantity(1.0),
            "edge": "stop_loss",
        },
        "t02_tp_only": {
            "bid": np.array([100.0, 103.0], dtype=np.float64),
            "ask": np.array([100.0, 103.0], dtype=np.float64),
            "direction": np.array([1, 0], dtype=np.int8),
            "sl_points": np.array([2.0, 2.0], dtype=np.float64),
            "tp_points": np.array([3.0, 3.0], dtype=np.float64),
            "initial_capital": 100_000.0,
            "point_value": 1.0,
            "sizing": _fixed_quantity(1.0),
            "edge": "take_profit",
        },
        "t03_opposite_signal_no_reentry": {
            "bid": np.array([10.0, 10.0, 10.0], dtype=np.float64),
            "ask": np.array([10.0, 10.0, 10.0], dtype=np.float64),
            "direction": np.array([1, -1, 0], dtype=np.int8),
            "sl_points": np.full(3, nan, dtype=np.float64),
            "tp_points": np.full(3, nan, dtype=np.float64),
            "initial_capital": 100_000.0,
            "point_value": 1.0,
            "sizing": _fixed_quantity(1.0),
            "edge": "signal_no_reentry",
        },
        "t04_long_spread_loss": {
            "bid": np.array([100.0, 100.0], dtype=np.float64),
            "ask": np.array([101.0, 101.0], dtype=np.float64),
            "direction": np.array([1, -1], dtype=np.int8),
            "sl_points": np.full(2, nan, dtype=np.float64),
            "tp_points": np.full(2, nan, dtype=np.float64),
            "initial_capital": 100_000.0,
            "point_value": 1.0,
            "sizing": _fixed_quantity(1.0),
            "edge": "long_spread_loss",
        },
        "t05_short_spread_loss": {
            "bid": np.array([100.0, 100.0], dtype=np.float64),
            "ask": np.array([101.0, 101.0], dtype=np.float64),
            "direction": np.array([-1, 1], dtype=np.int8),
            "sl_points": np.full(2, nan, dtype=np.float64),
            "tp_points": np.full(2, nan, dtype=np.float64),
            "initial_capital": 100_000.0,
            "point_value": 1.0,
            "sizing": _fixed_quantity(1.0),
            "edge": "short_spread_loss",
        },
        "t06_end_of_stream_close": {
            "bid": np.array([10.0, 11.0], dtype=np.float64),
            "ask": np.array([10.0, 11.0], dtype=np.float64),
            "direction": np.array([1, 0], dtype=np.int8),
            "sl_points": np.full(2, nan, dtype=np.float64),
            "tp_points": np.full(2, nan, dtype=np.float64),
            "initial_capital": 100_000.0,
            "point_value": 1.0,
            "sizing": _fixed_quantity(1.0),
            "edge": "end_of_day",
        },
        "t07_nan_levels": {
            "bid": np.array([100.0, 99.4, 99.4], dtype=np.float64),
            "ask": np.array([100.0, 99.4, 99.4], dtype=np.float64),
            "direction": np.array([1, 0, 0], dtype=np.int8),
            "sl_points": np.array([nan, 0.5, 0.5], dtype=np.float64),
            "tp_points": np.array([nan, 3.0, 3.0], dtype=np.float64),
            "initial_capital": 100_000.0,
            "point_value": 1.0,
            "sizing": _fixed_quantity(1.0),
            "edge": "nan_levels",
        },
        "t08_fractional_fixed_quantity": {
            "bid": np.array([100.0, 100.0], dtype=np.float64),
            "ask": np.array([100.0, 100.0], dtype=np.float64),
            "direction": np.array([1, -1], dtype=np.int8),
            "sl_points": np.full(2, nan, dtype=np.float64),
            "tp_points": np.full(2, nan, dtype=np.float64),
            "initial_capital": 100_000.0,
            "point_value": 1.0,
            "sizing": _fixed_quantity(1.5),
            "edge": "fractional_qty",
        },
        "t09_safety_margin_min_max": {
            "bid": np.array([10.0, 10.0, 20.0, 20.0, 30.0, 30.0], dtype=np.float64),
            "ask": np.array([10.0, 10.0, 20.0, 20.0, 30.0, 30.0], dtype=np.float64),
            "direction": np.array([1, -1, 1, -1, 1, -1], dtype=np.int8),
            "sl_points": np.full(6, nan, dtype=np.float64),
            "tp_points": np.full(6, nan, dtype=np.float64),
            "initial_capital": 10_000.0,
            "point_value": 1.0,
            "sizing": _safety_margin(2_000.0, min_contracts=1, max_contracts=3),
            "edge": "safety_margin_cap",
        },
        "t10_negative_capital": {
            "bid": np.array([100.0, 50.0, 50.0, 50.0], dtype=np.float64),
            "ask": np.array([100.0, 50.0, 50.0, 50.0], dtype=np.float64),
            "direction": np.array([1, -1, 1, -1], dtype=np.int8),
            "sl_points": np.full(4, nan, dtype=np.float64),
            "tp_points": np.full(4, nan, dtype=np.float64),
            "initial_capital": 100.0,
            "point_value": 10.0,  # (50-100)*1*10 → capital -400, then min-contracts re-entry
            "sizing": _safety_margin(2_000.0, min_contracts=1, max_contracts=None),
            "edge": "negative_capital",
        },
    }


def _day_bound_cases() -> list[dict[str, Any]]:
    return [
        {
            "case_id": "multi_day",
            "time_msc": np.array([0, DAY_MS, 2 * DAY_MS], dtype=np.int64),
        },
        {
            "case_id": "single_day",
            "time_msc": np.array([1_700_000_000_000, 1_700_000_000_000 + 40_000], dtype=np.int64),
        },
        {
            "case_id": "empty",
            "time_msc": np.asarray([], dtype=np.int64),
        },
        {
            "case_id": "pre_1970",
            "time_msc": np.array([-1], dtype=np.int64),
        },
    ]


def _assert_simulate_edge(scenario_id: str, edge: str, encoded: dict[str, object]) -> None:
    """Fail export loudly if a named edge scenario does not show its edge."""
    from q_backend.backtesting.tick.orders import ExitReason

    reasons = encoded["exit_reason"]["values"]  # type: ignore[index]
    assert isinstance(reasons, list)
    quantities = encoded["quantity"]["values"]  # type: ignore[index]
    assert isinstance(quantities, list)
    final_capital = float(encoded["final_capital"])  # type: ignore[arg-type]

    if edge == "stop_loss":
        if ExitReason.STOP_LOSS not in reasons:
            raise RuntimeError(f"{scenario_id}: expected STOP_LOSS in {reasons}")
    elif edge == "take_profit":
        if ExitReason.TAKE_PROFIT not in reasons:
            raise RuntimeError(f"{scenario_id}: expected TAKE_PROFIT in {reasons}")
    elif edge == "signal_no_reentry":
        if reasons != [int(ExitReason.SIGNAL)]:
            raise RuntimeError(f"{scenario_id}: expected single SIGNAL trade, got {reasons}")
    elif edge in ("long_spread_loss", "short_spread_loss"):
        if final_capital >= 100_000.0:
            raise RuntimeError(f"{scenario_id}: expected spread loss, final_capital={final_capital}")
    elif edge == "end_of_day":
        if ExitReason.END_OF_DAY not in reasons:
            raise RuntimeError(f"{scenario_id}: expected END_OF_DAY in {reasons}")
    elif edge == "nan_levels":
        if ExitReason.STOP_LOSS in reasons or ExitReason.TAKE_PROFIT in reasons:
            raise RuntimeError(f"{scenario_id}: NaN entry levels must not stop/target; got {reasons}")
        if ExitReason.END_OF_DAY not in reasons:
            raise RuntimeError(f"{scenario_id}: expected END_OF_DAY close, got {reasons}")
    elif edge == "fractional_qty":
        if quantities != [1.5]:
            raise RuntimeError(f"{scenario_id}: expected quantity 1.5, got {quantities}")
    elif edge == "safety_margin_cap":
        if not quantities or max(float(q) for q in quantities) > 3.0:
            raise RuntimeError(f"{scenario_id}: expected qty capped at 3, got {quantities}")
        if any(float(q) < 1.0 for q in quantities):
            raise RuntimeError(f"{scenario_id}: expected min contracts ≥1, got {quantities}")
    elif edge == "negative_capital":
        if final_capital >= 0.0:
            raise RuntimeError(f"{scenario_id}: expected negative capital, got {final_capital}")
        if len(quantities) < 2:
            raise RuntimeError(f"{scenario_id}: expected a second trade after capital went negative")
    else:
        raise RuntimeError(f"{scenario_id}: unknown edge {edge!r}")


def _run_simulate(spec: dict[str, Any]) -> tuple[Any, ...]:
    from q_backend.backtesting.position_sizing import (
        FixedQuantityPositionSizing,
        FixedSafetyMarginPositionSizing,
    )
    from q_backend.backtesting.tick.kernel import simulate
    from q_backend.backtesting.tick.orders import kernel_sizing_params

    sizing = spec["sizing"]
    if sizing["type"] == "fixed_quantity":
        config = FixedQuantityPositionSizing.model_validate(sizing)
    else:
        config = FixedSafetyMarginPositionSizing.model_validate(sizing)
    mode, a, b, c = kernel_sizing_params(config)
    return simulate(
        np.asarray(spec["bid"], dtype=np.float64),
        np.asarray(spec["ask"], dtype=np.float64),
        np.asarray(spec["direction"], dtype=np.int8),
        np.asarray(spec["sl_points"], dtype=np.float64),
        np.asarray(spec["tp_points"], dtype=np.float64),
        float(spec["initial_capital"]),
        float(spec["point_value"]),
        mode,
        a,
        b,
        c,
    )


def _synthetic_stream_case(source: BackendSource) -> dict[str, Any]:
    from q_backend.backtesting.tick.strategies.tick_ma_breakout import TickMaBreakoutStrategy
    from q_backend.backtesting.tick.strategy import TickArrays

    # Mirror test_goldens.synthetic_ticks defaults (AST load cannot see TickArrays).
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
    _ = source  # pin checkout available for provenance; arrays built locally
    return {
        "bid": ticks.bid,
        "ask": ticks.ask,
        "direction": signals.direction,
        "sl_points": signals.sl_points,
        "tp_points": signals.tp_points,
        "initial_capital": 100_000.0,
        "point_value": 1.0,
        "sizing": _fixed_quantity(1.0),
    }


def _encode_simulate_inputs(spec: dict[str, Any]) -> dict[str, object]:
    return {
        "bid": encode_column(np.asarray(spec["bid"], dtype=np.float64)),
        "ask": encode_column(np.asarray(spec["ask"], dtype=np.float64)),
        "direction": encode_column(np.asarray(spec["direction"], dtype=np.int64)),
        "sl_points": encode_column(np.asarray(spec["sl_points"], dtype=np.float64)),
        "tp_points": encode_column(np.asarray(spec["tp_points"], dtype=np.float64)),
    }


def _simulate_case_payload(scenario_id: str, spec: dict[str, Any]) -> dict[str, object]:
    result = _run_simulate(spec)
    expected = encode_ledger(result)
    edge = spec.get("edge")
    if edge is not None:
        _assert_simulate_edge(scenario_id, str(edge), expected)
    return {
        "case_id": scenario_id,
        "params": {
            "initial_capital": float(spec["initial_capital"]),
            "point_value": float(spec["point_value"]),
            "sizing": dict(spec["sizing"]),
        },
        "inputs": _encode_simulate_inputs(spec),
        "expected": expected,
    }


class TickKernelFamily:
    """Drive ``simulate`` and ``_split_ticks_by_day`` for tick_kernel fixtures."""

    name: Final[str] = "tick_kernel"
    environment: Final[Literal["numeric", "backend"]] = "backend"
    policy: Final[dict[str, object]] = {"kind": "exact"}

    def export(self, source: BackendSource, out_dir: Path) -> list[str]:
        from q_backend.backtesting.tick.engine import _split_ticks_by_day
        from q_backend.backtesting.tick.strategy import TickArrays

        sources = [
            {
                "path": "src/q_backend/backtesting/tick/kernel.py",
                "blob": git_blob_id(source, "src/q_backend/backtesting/tick/kernel.py"),
            },
            {
                "path": "src/q_backend/backtesting/tick/engine.py",
                "blob": git_blob_id(source, "src/q_backend/backtesting/tick/engine.py"),
            },
            {
                "path": "src/q_backend/backtesting/tick/orders.py",
                "blob": git_blob_id(source, "src/q_backend/backtesting/tick/orders.py"),
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

        simulate_specs = _simulate_scenarios()
        for scenario_id in SCENARIO_IDS:
            if scenario_id == "t12_day_bounds":
                cases: list[dict[str, object]] = []
                for day_case in _day_bound_cases():
                    time_msc = np.asarray(day_case["time_msc"], dtype=np.int64)
                    n = len(time_msc)
                    ticks = TickArrays(
                        time_msc=time_msc,
                        bid=np.zeros(n, dtype=np.float64),
                        ask=np.zeros(n, dtype=np.float64),
                        last=np.zeros(n, dtype=np.float64),
                        volume=np.zeros(n, dtype=np.float64),
                    )
                    chunks = _split_ticks_by_day(ticks)
                    starts, ends = day_bounds_from_chunks(chunks)
                    cases.append(
                        {
                            "case_id": f"{scenario_id}/{day_case['case_id']}",
                            "params": {},
                            "inputs": {"time_msc": encode_column(time_msc)},
                            "expected": encode_day_bounds(starts, ends),
                        }
                    )
                if len(cases) != 4:
                    raise RuntimeError(f"{scenario_id}: expected 4 day-bound cases, got {len(cases)}")
            elif scenario_id == "t11_synthetic_stream":
                spec = _synthetic_stream_case(source)
                cases = [_simulate_case_payload(scenario_id, spec)]
                reasons = cases[0]["expected"]["exit_reason"]["values"]  # type: ignore[index]
                if not reasons:
                    raise RuntimeError(f"{scenario_id}: synthetic stream produced no trades")
            else:
                cases = [_simulate_case_payload(scenario_id, simulate_specs[scenario_id])]

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
            sys.stderr.write(f"exported tick_kernel/{scenario_id}\n")

        return exported
