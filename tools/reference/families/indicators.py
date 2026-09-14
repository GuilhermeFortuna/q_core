"""Indicators fixture family."""

from __future__ import annotations

import importlib.metadata
import platform
from collections.abc import Callable
from dataclasses import dataclass
from pathlib import Path
from types import ModuleType
from typing import Final, Literal

import numpy as np
import pandas as pd

from export_reference import (
    ABS_TOL,
    REL_TOL,
    BackendSource,
    cpu_feature_level,
    dumps_fixture,
    encode_column,
    git_blob_id,
    load_generator,
    load_reference_module,
)


@dataclass(frozen=True)
class FunctionSpec:
    function_id: str  # file stem
    module_path: str  # backend-relative source path
    callable_name: str
    input_columns: tuple[str, ...]  # positional argument order, e.g. ("high", "low", "close")
    outputs: tuple[str, ...]  # names for returned Series / tuple members, in return order
    param_grid: tuple[dict[str, object], ...]
    fixed_kwargs: dict[str, object]  # e.g. {"ma_type": "hma"}


def build_inputs(generator: Callable[..., pd.DataFrame]) -> dict[str, pd.DataFrame]:
    """Build the 5 reference input datasets."""
    # 1. synthetic_ohlcv_n400
    df_synthetic = generator()

    # 2. constant_n60
    idx_60 = pd.date_range("2023-01-02", periods=60, freq="h", tz="UTC")
    df_constant = pd.DataFrame(
        {
            "open": np.full(60, 100.0),
            "high": np.full(60, 100.0),
            "low": np.full(60, 100.0),
            "close": np.full(60, 100.0),
            "volume": np.full(60, 1000.0),
        },
        index=idx_60,
    )

    # 3. monotonic_up_n60
    # close = 100 + i; open = close - 0.5; high = close + 0.25; low = open - 0.25; volume = 1000.0
    close_up = 100.0 + np.arange(60, dtype=float)
    open_up = close_up - 0.5
    high_up = close_up + 0.25
    low_up = open_up - 0.25
    df_monotonic_up = pd.DataFrame(
        {
            "open": open_up,
            "high": high_up,
            "low": low_up,
            "close": close_up,
            "volume": np.full(60, 1000.0),
        },
        index=idx_60,
    )

    # 4. nan_gaps_n120: rows 0..120 with NaN at rows 0, 17, 18, 19, 64 in every column
    df_nan_gaps = df_synthetic.iloc[:120].copy()
    for col in ("open", "high", "low", "close", "volume"):
        df_nan_gaps.iloc[[0, 17, 18, 19, 64], df_nan_gaps.columns.get_loc(col)] = np.nan

    # 5. short_n3: rows 0..3
    df_short = df_synthetic.iloc[:3].copy()

    return {
        "synthetic_ohlcv_n400": df_synthetic,
        "constant_n60": df_constant,
        "monotonic_up_n60": df_monotonic_up,
        "nan_gaps_n120": df_nan_gaps,
        "short_n3": df_short,
    }


def function_specs() -> tuple[FunctionSpec, ...]:
    """Define specs for the 16 ported indicator and transform functions."""
    ma_periods: tuple[dict[str, object], ...] = (
        {"period": 0},
        {"period": 1},
        {"period": 2},
        {"period": 5},
        {"period": 20},
        {"period": 21},
        {"period": 100},
    )

    return (
        FunctionSpec(
            function_id="realized_vol",
            module_path="src/q_backend/backtesting/technical_indicators.py",
            callable_name="compute_realized_vol",
            input_columns=("close",),
            outputs=("realized_vol",),
            param_grid=(
                {"periods_per_year": 252, "window": 1},
                {"periods_per_year": 252, "window": 2},
                {"periods_per_year": 252, "window": 20},
                {"periods_per_year": 252, "window": 60},
            ),
            fixed_kwargs={},
        ),
        FunctionSpec(
            function_id="yang_zhang",
            module_path="src/q_backend/backtesting/technical_indicators.py",
            callable_name="compute_yang_zhang",
            input_columns=("open", "high", "low", "close"),
            outputs=("yang_zhang",),
            param_grid=(
                {"periods_per_year": 252, "window": 1},
                {"periods_per_year": 252, "window": 2},
                {"periods_per_year": 252, "window": 20},
                {"periods_per_year": 252, "window": 60},
            ),
            fixed_kwargs={},
        ),
        FunctionSpec(
            function_id="rsi",
            module_path="src/q_backend/backtesting/technical_indicators.py",
            callable_name="compute_rsi",
            input_columns=("close",),
            outputs=("rsi",),
            param_grid=(
                {"period": 0},
                {"period": 1},
                {"period": 2},
                {"period": 14},
                {"period": 100},
            ),
            fixed_kwargs={},
        ),
        FunctionSpec(
            function_id="bollinger_bands",
            module_path="src/q_backend/backtesting/technical_indicators.py",
            callable_name="compute_bollinger_bands",
            input_columns=("close",),
            outputs=("upper", "middle", "lower"),
            param_grid=(
                {"num_std": 2.0, "period": 1},
                {"num_std": 1.0, "period": 2},
                {"num_std": 0.0, "period": 20},
                {"num_std": 2.0, "period": 20},
                {"num_std": 2.5, "period": 20},
            ),
            fixed_kwargs={},
        ),
        FunctionSpec(
            function_id="macd",
            module_path="src/q_backend/backtesting/technical_indicators.py",
            callable_name="compute_macd",
            input_columns=("close",),
            outputs=("line", "signal", "histogram"),
            param_grid=(
                {"fast_period": 1, "signal_period": 1, "slow_period": 1},
                {"fast_period": 3, "signal_period": 2, "slow_period": 6},
                {"fast_period": 12, "signal_period": 9, "slow_period": 26},
                {"fast_period": 26, "signal_period": 9, "slow_period": 12},
            ),
            fixed_kwargs={},
        ),
        FunctionSpec(
            function_id="donchian_channels",
            module_path="src/q_backend/backtesting/technical_indicators.py",
            callable_name="compute_donchian_channels",
            input_columns=("high", "low"),
            outputs=("upper", "lower"),
            param_grid=(
                {"period": 0},
                {"period": 1},
                {"period": 20},
                {"period": 100},
            ),
            fixed_kwargs={},
        ),
        FunctionSpec(
            function_id="atr",
            module_path="src/q_backend/backtesting/technical_indicators.py",
            callable_name="compute_atr",
            input_columns=("high", "low", "close"),
            outputs=("atr",),
            param_grid=(
                {"period": 0},
                {"period": 1},
                {"period": 2},
                {"period": 14},
                {"period": 100},
            ),
            fixed_kwargs={},
        ),
        FunctionSpec(
            function_id="ma_sma",
            module_path="src/q_backend/backtesting/moving_averages.py",
            callable_name="compute_ma",
            input_columns=("close",),
            outputs=("ma_sma",),
            param_grid=ma_periods,
            fixed_kwargs={"ma_type": "sma"},
        ),
        FunctionSpec(
            function_id="ma_ema",
            module_path="src/q_backend/backtesting/moving_averages.py",
            callable_name="compute_ma",
            input_columns=("close",),
            outputs=("ma_ema",),
            param_grid=ma_periods,
            fixed_kwargs={"ma_type": "ema"},
        ),
        FunctionSpec(
            function_id="ma_smma",
            module_path="src/q_backend/backtesting/moving_averages.py",
            callable_name="compute_ma",
            input_columns=("close",),
            outputs=("ma_smma",),
            param_grid=ma_periods,
            fixed_kwargs={"ma_type": "smma"},
        ),
        FunctionSpec(
            function_id="ma_wma",
            module_path="src/q_backend/backtesting/moving_averages.py",
            callable_name="compute_ma",
            input_columns=("close",),
            outputs=("ma_wma",),
            param_grid=ma_periods,
            fixed_kwargs={"ma_type": "wma"},
        ),
        FunctionSpec(
            function_id="ma_hma",
            module_path="src/q_backend/backtesting/moving_averages.py",
            callable_name="compute_ma",
            input_columns=("close",),
            outputs=("ma_hma",),
            param_grid=ma_periods,
            fixed_kwargs={"ma_type": "hma"},
        ),
        FunctionSpec(
            function_id="rolling_zscore",
            module_path="src/q_backend/backtesting/transforms.py",
            callable_name="compute_rolling_zscore",
            input_columns=("close",),
            outputs=("rolling_zscore",),
            param_grid=(
                {"window": 1},
                {"window": 2},
                {"window": 20},
                {"window": 100},
            ),
            fixed_kwargs={},
        ),
        FunctionSpec(
            function_id="rolling_rank",
            module_path="src/q_backend/backtesting/transforms.py",
            callable_name="compute_rolling_rank",
            input_columns=("close",),
            outputs=("rolling_rank",),
            param_grid=(
                {"window": 0},
                {"window": 1},
                {"window": 20},
                {"window": 100},
            ),
            fixed_kwargs={},
        ),
        FunctionSpec(
            function_id="pct_change",
            module_path="src/q_backend/backtesting/transforms.py",
            callable_name="compute_pct_change",
            input_columns=("close",),
            outputs=("pct_change",),
            param_grid=(
                {"change_bars": 0},
                {"change_bars": 1},
                {"change_bars": 5},
                {"change_bars": 100},
            ),
            fixed_kwargs={},
        ),
        FunctionSpec(
            function_id="clip",
            module_path="src/q_backend/backtesting/transforms.py",
            callable_name="compute_clip",
            input_columns=("close",),
            outputs=("clip",),
            param_grid=(
                {"high": 105.0, "low": 95.0},
                {"high": 100.0, "low": 100.0},
                {"high": 95.0, "low": 105.0},
                {"high": 1e300, "low": -1e300},
            ),
            fixed_kwargs={},
        ),
    )


def run_case(
    spec: FunctionSpec,
    module: ModuleType,
    frame: pd.DataFrame,
    params: dict[str, object],
) -> dict[str, object]:
    """Execute a single test case for a function spec."""
    fn = getattr(module, spec.callable_name)
    args = [frame[col] for col in spec.input_columns]
    kwargs = dict(spec.fixed_kwargs)
    kwargs.update(params)

    # compute_clip has parameter names clip_low and clip_high
    if spec.callable_name == "compute_clip":
        low_val = kwargs.pop("low", kwargs.get("clip_low"))
        high_val = kwargs.pop("high", kwargs.get("clip_high"))
        kwargs["clip_low"] = low_val
        kwargs["clip_high"] = high_val

    try:
        res = fn(*args, **kwargs)
    except Exception as exc:
        return {"rejected": {"python_exception": type(exc).__name__}}

    if len(spec.outputs) == 1:
        return {"outputs": {spec.outputs[0]: encode_column(res)}}
    else:
        return {"outputs": {name: encode_column(s) for name, s in zip(spec.outputs, res)}}


class _SeriesWrapper:
    def __init__(self, series: pd.Series):
        self.series = series


def check_reference_causal(
    spec: FunctionSpec,
    module: ModuleType,
    frame: pd.DataFrame,
    params: dict[str, object],
    leakage: ModuleType,
) -> int:
    """Run assert_causal on the reference implementation at sampled indices."""
    fn = getattr(module, spec.callable_name)
    n = len(frame)
    if n <= 120:
        sample_indices = list(range(n))
    else:
        sample_indices = list(range(121)) + list(range(130, n, 10))
        if (n - 1) not in sample_indices:
            sample_indices.append(n - 1)
        sample_indices = sorted(set(sample_indices))

    kwargs = dict(spec.fixed_kwargs)
    kwargs.update(params)
    if spec.callable_name == "compute_clip":
        low_val = kwargs.pop("low", kwargs.get("clip_low"))
        high_val = kwargs.pop("high", kwargs.get("clip_high"))
        kwargs["clip_low"] = low_val
        kwargs["clip_high"] = high_val

    for out_idx in range(len(spec.outputs)):
        def compute_single(df_slice: pd.DataFrame) -> _SeriesWrapper:
            args = [df_slice[col] for col in spec.input_columns]
            res = fn(*args, **kwargs)
            if len(spec.outputs) == 1:
                return _SeriesWrapper(res)
            return _SeriesWrapper(res[out_idx])

        leakage.assert_causal(compute_single, frame, sample_indices=sample_indices)

    return len(sample_indices)


class IndicatorFamily:
    """Family of technical indicators and series transforms."""

    name: Final[str] = "indicators"
    environment: Final[Literal["numeric", "backend"]] = "numeric"
    policy: Final[dict[str, object]] = {"kind": "abs_rel_tol", "abs": ABS_TOL, "rel": REL_TOL}

    def export(self, source: BackendSource, out_dir: Path) -> list[str]:
        """Export inputs and indicator fixtures to out_dir."""
        inputs_dir = out_dir / "inputs"
        indicators_dir = out_dir / self.name
        inputs_dir.mkdir(parents=True, exist_ok=True)
        indicators_dir.mkdir(parents=True, exist_ok=True)

        generator = load_generator(source, "tests/backtesting/test_goldens.py", "synthetic_ohlcv")
        goldens_blob = git_blob_id(source, "tests/backtesting/test_goldens.py")
        input_frames = build_inputs(generator)

        constructions = {
            "synthetic_ohlcv_n400": "Synthetic OHLCV series generated with default parameters",
            "constant_n60": "Constant series of 60 bars with price 100.0 and volume 1000.0",
            "monotonic_up_n60": "Strictly increasing series of 60 bars with price starting at 100.0 and volume 1000.0",
            "nan_gaps_n120": "First 120 bars of synthetic OHLCV with NaN at rows 0, 17, 18, 19, 64",
            "short_n3": "First 3 bars of synthetic OHLCV",
        }

        # Write inputs
        for input_id, frame in input_frames.items():
            input_file = inputs_dir / f"{input_id}.json"
            kwargs: dict[str, object] = {}
            if "synthetic" in input_id or "nan_gaps" in input_id or "short" in input_id:
                kwargs = {"freq": "h", "n": 400, "seed": 20240609, "start": "2023-01-02"}
            input_payload = {
                "columns": {col: encode_column(frame[col].to_numpy(dtype=float)) for col in ("open", "high", "low", "close", "volume")},
                "format": "q-core-reference-fixture/1",
                "input_id": input_id,
                "provenance": {
                    "backend_repo": source.repo_url,
                    "backend_rev": source.rev,
                    "construction": constructions[input_id],
                    "exporter": "tools/reference/export_reference.py",
                    "generator": {
                        "blob": goldens_blob,
                        "kwargs": kwargs,
                        "name": "synthetic_ohlcv",
                        "path": "tests/backtesting/test_goldens.py",
                    },
                },
            }
            input_file.write_text(dumps_fixture(input_payload), encoding="utf-8")

        # Load reference modules
        modules: dict[str, ModuleType] = {
            "src/q_backend/backtesting/technical_indicators.py": load_reference_module(
                source, "src/q_backend/backtesting/technical_indicators.py"
            ),
            "src/q_backend/backtesting/moving_averages.py": load_reference_module(
                source, "src/q_backend/backtesting/moving_averages.py"
            ),
            "src/q_backend/backtesting/transforms.py": load_reference_module(
                source, "src/q_backend/backtesting/transforms.py"
            ),
        }
        leakage_mod = load_reference_module(source, "src/q_backend/features/leakage.py")

        exported_ids: list[str] = []
        cpu_level = cpu_feature_level()
        py_ver = platform.python_version()
        np_ver = importlib.metadata.version("numpy")
        pd_ver = importlib.metadata.version("pandas")

        for spec in function_specs():
            mod = modules[spec.module_path]
            blob = git_blob_id(source, spec.module_path)
            total_causality_checked = 0
            cases: list[dict[str, object]] = []

            for input_id, frame in input_frames.items():
                for params in spec.param_grid:
                    param_parts = [f"{k}={params[k]}" for k in sorted(params.keys())]
                    param_str = ",".join(param_parts)
                    case_id = f"{input_id}/{param_str}" if param_str else input_id

                    expected = run_case(spec, mod, frame, params)
                    if "outputs" in expected:
                        indices_checked = check_reference_causal(spec, mod, frame, params, leakage_mod)
                        total_causality_checked += indices_checked

                    case_inputs = {col: f"{input_id}.{col}" for col in spec.input_columns}
                    cases.append(
                        {
                            "case_id": case_id,
                            "expected": expected,
                            "inputs": case_inputs,
                            "params": dict(params),
                        }
                    )

            src_dict: dict[str, object] = {
                "blob": blob,
                "callable": spec.callable_name,
                "path": spec.module_path,
            }
            if "ma_type" in spec.fixed_kwargs:
                src_dict["ma_type"] = spec.fixed_kwargs["ma_type"]

            fixture_payload = {
                "cases": cases,
                "family": self.name,
                "fixture_id": spec.function_id,
                "format": "q-core-reference-fixture/1",
                "policy": dict(self.policy),
                "provenance": {
                    "backend_repo": source.repo_url,
                    "backend_rev": source.rev,
                    "causality_checked_indices": total_causality_checked,
                    "cpu_level": cpu_level,
                    "environment": self.environment,
                    "exporter": "tools/reference/export_reference.py",
                    "numpy": np_ver,
                    "pandas": pd_ver,
                    "python": py_ver,
                    "source": src_dict,
                },
            }

            fixture_file = indicators_dir / f"{spec.function_id}.json"
            fixture_file.write_text(dumps_fixture(fixture_payload), encoding="utf-8")
            exported_ids.append(spec.function_id)

        return exported_ids
