"""Exit-rules reference family: ExitStrategy.check_exits parity with per-bar state."""

from __future__ import annotations

import importlib.metadata
import math
import platform
import sys
from dataclasses import dataclass
from datetime import datetime
from pathlib import Path
from typing import Any, Final, Literal

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

# ExitRuleId discriminants — must match q_engine::exits::ExitRuleId.
RULE_CODE: Final[dict[str, int]] = {
    "fixed_sl": 0,
    "atr_sl": 1,
    "fixed_tp": 2,
    "atr_tp": 3,
    "trailing": 4,
    "chandelier": 5,
    "breakeven": 6,
    "psar": 7,
    "profit_target_ratchet": 8,
    "time_stop": 9,
    "donchian_stop": 10,
}

SCENARIO_IDS: Final = (
    "r01_fixed_sl",
    "r02_atr_sl",
    "r03_fixed_tp",
    "r04_atr_tp",
    "r05_trailing",
    "r06_chandelier",
    "r07_breakeven",
    "r08_psar",
    "r09_profit_target_ratchet",
    "r10_time_stop",
    "r11_donchian_stop",
    "c01_all_rules",
    "c02_close_only",
    "c03_multi_positions_reuse",
    "c04_psar_cap",
    "c05_time_stop_one",
)

_STATE_FLOATS: Final = (
    "trailing_extreme",
    "chandelier_peak",
    "psar_sar",
    "psar_ep",
    "psar_af",
    "psar_prior",
    "psar_prior_prior",
    "ratchet",
)
_STATE_INTS: Final = ("breakeven_armed", "psar_present", "time_stop_bars")


@dataclass(frozen=True)
class PositionSpec:
    key: int
    side: Literal["long", "short"]
    entry_price: float
    first_bar: int
    last_bar: int  # inclusive


def encode_schedule(positions: list[PositionSpec]) -> dict[str, object]:
    """Encode the open-position schedule; reject inverted intervals."""
    for p in positions:
        if p.first_bar > p.last_bar:
            raise ValueError(
                f"position key={p.key}: first_bar {p.first_bar} after last_bar {p.last_bar}"
            )
    return {
        "key": encode_column(np.asarray([p.key for p in positions], dtype=np.int64)),
        "side": encode_column(
            np.asarray([0 if p.side == "long" else 1 for p in positions], dtype=np.int64)
        ),
        "entry_price": encode_column(
            np.asarray([p.entry_price for p in positions], dtype=np.float64)
        ),
        "first_bar": encode_column(
            np.asarray([p.first_bar for p in positions], dtype=np.int64)
        ),
        "last_bar": encode_column(
            np.asarray([p.last_bar for p in positions], dtype=np.int64)
        ),
    }


def encode_rule_state(rule_states: dict[str, dict[str, Any]] | None) -> dict[str, float | int]:
    """Map ExitStrategy._state[trade] rule dicts into flat RuleState scalars.

    Absent optional floats become NaN; absent int/flag fields become -1.
    """
    rs = rule_states or {}
    trailing = rs.get("trailing") or {}
    chandelier = rs.get("chandelier") or {}
    breakeven = rs.get("breakeven") or {}
    psar = rs.get("psar") or {}
    ratchet = rs.get("profit_target_ratchet") or {}
    time_stop = rs.get("time_stop") or {}

    trailing_extreme = float(trailing["extreme"]) if "extreme" in trailing else float("nan")
    chandelier_peak = float(chandelier["peak"]) if "peak" in chandelier else float("nan")
    if "armed" in breakeven:
        breakeven_armed = 1 if breakeven.get("armed") else 0
    else:
        breakeven_armed = -1

    if "sar" in psar:
        prior = psar.get("prior_low", psar.get("prior_high"))
        prior_prior = psar.get("prior_prior_low", psar.get("prior_prior_high"))
        psar_present = 1
        psar_sar = float(psar["sar"])
        psar_ep = float(psar["ep"])
        psar_af = float(psar["af"])
        psar_prior = float(prior) if prior is not None else float("nan")
        psar_prior_prior = float(prior_prior) if prior_prior is not None else float("nan")
    else:
        psar_present = -1
        psar_sar = psar_ep = psar_af = psar_prior = psar_prior_prior = float("nan")

    if ratchet.get("armed") and "ratchet" in ratchet:
        ratchet_level = float(ratchet["ratchet"])
    else:
        ratchet_level = float("nan")

    time_stop_bars = int(time_stop["bars"]) if "bars" in time_stop else -1

    return {
        "trailing_extreme": trailing_extreme,
        "chandelier_peak": chandelier_peak,
        "breakeven_armed": breakeven_armed,
        "psar_present": psar_present,
        "psar_sar": psar_sar,
        "psar_ep": psar_ep,
        "psar_af": psar_af,
        "psar_prior": psar_prior,
        "psar_prior_prior": psar_prior_prior,
        "ratchet": ratchet_level,
        "time_stop_bars": time_stop_bars,
    }


def _default_long_short(n: int) -> list[PositionSpec]:
    last = n - 1
    return [
        PositionSpec(1, "long", 100.0, 5, last),
        PositionSpec(2, "short", 100.0, 5, last),
    ]


def _scenario_params() -> dict[str, dict[str, Any]]:
    return {
        "r01_fixed_sl": {"stop_loss_pct": 0.01},
        "r02_atr_sl": {"stop_loss_atr": 0.25, "atr_period": 14},
        "r03_fixed_tp": {"take_profit_pct": 0.005},
        "r04_atr_tp": {"take_profit_atr": 0.25, "atr_period": 14},
        "r05_trailing": {"trailing_stop_pct": 0.003},
        "r06_chandelier": {"chandelier_atr_mult": 0.25, "atr_period": 14},
        "r07_breakeven": {
            "breakeven_trigger_pct": 0.002,
            "breakeven_offset_pct": 0.0,
        },
        "r08_psar": {
            "psar_af_start": 0.02,
            "psar_af_step": 0.02,
            "psar_af_max": 0.2,
        },
        "r09_profit_target_ratchet": {
            "target_ratchet_atr": 0.25,
            "atr_period": 14,
        },
        "r10_time_stop": {"max_bars_in_trade": 40},
        "r11_donchian_stop": {"donchian_exit_period": 20},
        "c01_all_rules": {
            "stop_loss_pct": 0.05,
            "stop_loss_atr": 3.0,
            "take_profit_pct": 0.08,
            "take_profit_atr": 4.0,
            "trailing_stop_pct": 0.04,
            "atr_period": 14,
            "chandelier_atr_mult": 4.0,
            "breakeven_trigger_pct": 0.03,
            "breakeven_offset_pct": 0.005,
            "psar_af_start": 0.02,
            "psar_af_step": 0.02,
            "psar_af_max": 0.2,
            "target_ratchet_atr": 3.0,
            "max_bars_in_trade": 80,
            "donchian_exit_period": 20,
        },
        "c02_close_only": {
            "stop_loss_pct": 0.03,
            "take_profit_pct": 0.05,
            "trailing_stop_pct": 0.02,
            "breakeven_trigger_pct": 0.02,
            "breakeven_offset_pct": 0.0,
            "psar_af_start": 0.02,
            "max_bars_in_trade": 60,
        },
        "c03_multi_positions_reuse": {
            "stop_loss_pct": 0.03,
            "trailing_stop_pct": 0.02,
            "max_bars_in_trade": 100,
        },
        "c04_psar_cap": {
            "psar_af_start": 0.02,
            "psar_af_step": 0.1,
            "psar_af_max": 0.25,
        },
        "c05_time_stop_one": {"max_bars_in_trade": 1},
    }


def _scenario_schedules(n: int) -> dict[str, list[PositionSpec]]:
    last = n - 1
    schedules: dict[str, list[PositionSpec]] = {
        sid: _default_long_short(n) for sid in SCENARIO_IDS if sid.startswith("r")
    }
    schedules["c01_all_rules"] = _default_long_short(n)
    schedules["c02_close_only"] = _default_long_short(n)
    schedules["c04_psar_cap"] = _default_long_short(n)
    schedules["c05_time_stop_one"] = _default_long_short(n)
    schedules["c03_multi_positions_reuse"] = [
        PositionSpec(1, "long", 100.0, 5, 59),
        PositionSpec(2, "short", 105.0, 20, 80),
        PositionSpec(3, "long", 98.0, 40, last),
        PositionSpec(1, "short", 110.0, 70, last),
    ]
    return schedules


def _make_trade(spec: PositionSpec, bar_time: datetime) -> Any:
    from q_backend.backtesting.models import OrderAction, Trade

    action = OrderAction.BUY if spec.side == "long" else OrderAction.SELL
    return Trade(
        id=str(spec.key),
        order_id=f"o-{spec.key}",
        symbol=f"T{spec.key}",
        action=action,
        quantity=1.0,
        entry_time=bar_time,
        entry_price=spec.entry_price,
    )


def _indicator_params(params: dict[str, Any]) -> dict[str, Any]:
    """Periods used when attaching indicator columns to the OHLC frame."""
    out = dict(params)
    out.setdefault("atr_period", 14)
    if int(out.get("donchian_exit_period", 0) or 0) <= 0:
        out["donchian_exit_period"] = 20
    return out


def _augment_frame(params: dict[str, Any], bars: pd.DataFrame) -> pd.DataFrame:
    """Attach atr_<period> and donchian_* columns the way the pin's engine would."""
    from q_backend.backtesting import technical_indicators

    frame = bars.copy()
    atr_period = int(params.get("atr_period", 14))
    atr_col = f"atr_{atr_period}"
    if atr_col not in frame.columns:
        frame[atr_col] = technical_indicators.compute_atr(
            frame["high"], frame["low"], frame["close"], atr_period
        )
    donchian_period = int(params.get("donchian_exit_period", 0) or 0)
    if donchian_period <= 0:
        donchian_period = 20
    high_col = f"donchian_high_{donchian_period}"
    low_col = f"donchian_low_{donchian_period}"
    if high_col not in frame.columns or low_col not in frame.columns:
        upper, lower = technical_indicators.compute_donchian_channels(
            frame["high"], frame["low"], donchian_period
        )
        frame[high_col] = upper
        frame[low_col] = lower
    return frame


def _open_for_bar(schedule: list[PositionSpec], bar: int) -> list[PositionSpec]:
    return [p for p in schedule if p.first_bar <= bar <= p.last_bar]


def _bar_row(frame: pd.DataFrame, bar: int, *, close_only: bool) -> pd.Series:
    row = frame.iloc[bar]
    if not close_only:
        return row
    data: dict[str, float] = {"close": float(row["close"])}
    for col in frame.columns:
        if col.startswith("atr_") or col.startswith("donchian_"):
            data[col] = float(row[col]) if pd.notna(row[col]) else float("nan")
    return pd.Series(data)


def _run_scenario(
    frame: pd.DataFrame,
    params: dict[str, Any],
    schedule: list[PositionSpec],
    *,
    close_only: bool,
) -> dict[str, object]:
    from q_backend.backtesting.exit_strategy import ExitStrategy

    n = len(frame)
    strategy = ExitStrategy(params)
    keys = sorted({p.key for p in schedule})

    exit_codes: dict[int, list[float]] = {k: [-1.0] * n for k in keys}
    state_f: dict[str, dict[int, list[float]]] = {
        name: {k: [float("nan")] * n for k in keys} for name in _STATE_FLOATS
    }
    state_i: dict[str, dict[int, list[int]]] = {
        name: {k: [-1] * n for k in keys} for name in _STATE_INTS
    }

    for bar in range(n):
        open_specs = _open_for_bar(schedule, bar)
        trades = [_make_trade(p, frame.index[bar].to_pydatetime()) for p in open_specs]
        row = _bar_row(frame, bar, close_only=close_only)
        signals = strategy.check_exits(trades, row)

        # Signals are in open-trades order for trades that exited only.
        exited_ids: list[str] = []
        # Reconstruct which trades exited by replaying the same first-rule-wins walk.
        # Prefer correlating by unique symbol.
        reason_by_symbol = {sig.symbol: sig.exit_reason for sig in signals}

        for p in open_specs:
            tid = str(p.key)
            reason = reason_by_symbol.get(f"T{p.key}")
            exit_codes[p.key][bar] = float(RULE_CODE[reason]) if reason else -1.0
            encoded = encode_rule_state(strategy._state.get(tid))
            for name in _STATE_FLOATS:
                state_f[name][p.key][bar] = float(encoded[name])
            for name in _STATE_INTS:
                state_i[name][p.key][bar] = int(encoded[name])
            if reason:
                exited_ids.append(tid)

    expected: dict[str, object] = {}
    for k in keys:
        expected[f"exit_code_{k}"] = encode_column(
            np.asarray(exit_codes[k], dtype=np.float64)
        )
        for name in _STATE_FLOATS:
            expected[f"{name}_{k}"] = encode_column(
                np.asarray(state_f[name][k], dtype=np.float64)
            )
        for name in _STATE_INTS:
            expected[f"{name}_{k}"] = encode_column(
                np.asarray(state_i[name][k], dtype=np.int64)
            )
    return expected


def _exits_per_side(
    schedule: list[PositionSpec], expected: dict[str, object]
) -> dict[str, int]:
    """Count schedule intervals that saw at least one exit, by side."""
    counts = {"long": 0, "short": 0}
    for idx, p in enumerate(schedule):
        col = expected[f"exit_code_{p.key}"]
        assert isinstance(col, dict)
        values = col["values"]
        assert isinstance(values, list)
        # For reused keys, only count exits within this interval.
        window = values[p.first_bar : p.last_bar + 1]
        fired = any(
            isinstance(v, (int, float))
            and not (isinstance(v, float) and math.isnan(float(v)))
            and int(v) >= 0
            for v in window
        )
        if fired:
            counts[p.side] += 1
        _ = idx
    return counts


class ExitRulesFamily:
    """Drive ExitStrategy.check_exits with a scripted open-position schedule."""

    name: Final[str] = "exit_rules"
    environment: Final[Literal["numeric", "backend"]] = "backend"
    policy: Final[dict[str, object]] = {"kind": "exact"}

    def export(self, source: BackendSource, out_dir: Path) -> list[str]:
        generator = load_generator(
            source, "tests/backtesting/test_goldens.py", "synthetic_ohlcv"
        )
        bars = generator(n=160)
        params_by_id = _scenario_params()
        schedules = _scenario_schedules(len(bars))

        sources = [
            {
                "path": "src/q_backend/backtesting/exit_strategy.py",
                "blob": git_blob_id(
                    source, "src/q_backend/backtesting/exit_strategy.py"
                ),
            },
            {
                "path": "src/q_backend/backtesting/exit_rules/registry.py",
                "blob": git_blob_id(
                    source, "src/q_backend/backtesting/exit_rules/registry.py"
                ),
            },
            {
                "path": "src/q_backend/backtesting/exit_rules/legacy.py",
                "blob": git_blob_id(
                    source, "src/q_backend/backtesting/exit_rules/legacy.py"
                ),
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
            params = params_by_id[scenario_id]
            schedule = list(schedules[scenario_id])
            # Pin entry to the open price at first_bar so levels sit on the series.
            schedule = [
                PositionSpec(
                    p.key,
                    p.side,
                    float(bars.iloc[p.first_bar]["close"]),
                    p.first_bar,
                    p.last_bar,
                )
                for p in schedule
            ]
            close_only = scenario_id == "c02_close_only"
            frame = _augment_frame(_indicator_params(params), bars)
            expected = _run_scenario(
                frame, params, schedule, close_only=close_only
            )
            counts = _exits_per_side(schedule, expected)
            if scenario_id.startswith("r") and (
                counts["long"] < 1 or counts["short"] < 1
            ):
                raise RuntimeError(
                    f"{scenario_id} needs ≥1 exit per side; got {counts}. "
                    "Re-parameterise before committing fixtures."
                )

            inputs: dict[str, object] = {
                "close": encode_column(frame["close"].to_numpy(dtype=np.float64)),
            }
            if not close_only:
                inputs["open"] = encode_column(
                    frame["open"].to_numpy(dtype=np.float64)
                )
                inputs["high"] = encode_column(
                    frame["high"].to_numpy(dtype=np.float64)
                )
                inputs["low"] = encode_column(frame["low"].to_numpy(dtype=np.float64))
            for col in frame.columns:
                if col.startswith("atr_") or col.startswith("donchian_"):
                    inputs[col] = encode_column(
                        frame[col].to_numpy(dtype=np.float64)
                    )

            payload = {
                "format": FORMAT,
                "family": self.name,
                "fixture_id": scenario_id,
                "policy": dict(self.policy),
                "provenance": provenance,
                "cases": [
                    {
                        "case_id": scenario_id,
                        "params": params,
                        "schedule": encode_schedule(schedule),
                        "inputs": inputs,
                        "expected": expected,
                    }
                ],
            }
            path = family_dir / f"{scenario_id}.json"
            path.write_text(dumps_fixture(payload), encoding="utf-8")
            exported.append(scenario_id)
            sys.stderr.write(f"exported exit_rules/{scenario_id} exits={counts}\n")

        return exported
