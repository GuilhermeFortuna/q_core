"""Decision-step reference family: the queued signals one closed bar produces.

Section D of the engine loop is shared with forward execution. Each scenario records
`reference_queued_signals_by_close` bar by bar and asserts, at export time, that
`StrategyEvaluator.ingest_completed_bars` agrees on every bar exactly as
`test_backtest_live_parity` does. The evaluator's `requested_quantity` is recorded too, so
sizing at the bar's close with the initial capital is pinned alongside the decision.
"""

from __future__ import annotations

import copy
import sys
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Final, Literal

import numpy as np
import pandas as pd

from export_reference import (
    FORMAT,
    BackendSource,
    dumps_fixture,
    encode_column,
    load_generator,
)
from families.candle_engine import build_provenance, index_us, sizing_model, timestamp_us
from families.exit_rules import RULE_CODE
from families.scripted_strategy import (
    BAR_INDEX_COLUMN,
    ENTRY_COLUMN,
    EXIT_LONG_COLUMN,
    EXIT_SHORT_COLUMN,
    STRENGTH_COLUMN,
    SYMBOL,
    Decisions,
    build_scripted_strategy,
    scripted_decisions,
)

TIMEFRAME: Final = "H1"
INITIAL_CAPITAL: Final = 100_000.0
TRADE_ID: Final = "golden-parity-trade"

# A queued close with no rule behind it is a strategy or holding-period exit.
NO_RULE_CODE: Final = 11

SCENARIO_IDS: Final = (
    "d01_no_trade_inverse_volatility",
    "d02_long_open_scripted_exit",
    "d03_short_open_scripted_exit",
    "d04_trailing_atr_stop_long",
    "d05_psar_time_stop_short",
    "d06_holding_period_on_grid",
    "d07_holding_period_off_grid",
)

_OHLC: Final = ("open", "high", "low", "close")


def queued_reason_code(exit_reason: Any) -> int:
    """The fixture code for a queued close: its rule, or 11 when no rule set a reason."""
    if exit_reason is None:
        return NO_RULE_CODE
    name = str(exit_reason)
    if name not in RULE_CODE:
        raise ValueError(f"unknown queued exit reason {exit_reason!r}")
    return RULE_CODE[name]


def _entry_code(action: Any) -> int:
    name = str(getattr(action, "value", action))
    if name == "BUY":
        return 1
    if name == "SELL":
        return -1
    raise ValueError(f"unexpected entry action {action!r}")


def encode_decisions(
    rows: list[tuple[Any, list[Any], list[Any]]],
    requested_quantity: list[float],
) -> dict[str, object]:
    """Encode the per-bar queued exits and entry in `DecisionTrace`'s layout.

    `exit_reason[exit_offsets[i]:exit_offsets[i + 1]]` is bar `i`'s queued closes, so a bar
    that queues nothing contributes an empty range rather than a sentinel row.
    """
    exit_offsets: list[int] = [0]
    exit_reason: list[int] = []
    entry: list[int] = []
    entry_strength: list[float] = []
    for _, exits, entries in rows:
        for signal in exits:
            exit_reason.append(queued_reason_code(getattr(signal, "exit_reason", None)))
        exit_offsets.append(len(exit_reason))
        if entries:
            entry.append(_entry_code(entries[0].action))
            entry_strength.append(float(entries[0].strength))
        else:
            entry.append(0)
            entry_strength.append(0.0)
    return {
        "exit_offsets": encode_column(np.asarray(exit_offsets, dtype=np.int64)),
        "exit_reason": encode_column(np.asarray(exit_reason, dtype=np.int64)),
        "entry": encode_column(np.asarray(entry, dtype=np.int64)),
        "entry_strength": encode_column(np.asarray(entry_strength, dtype=np.float64)),
        "requested_quantity": encode_column(np.asarray(requested_quantity, dtype=np.float64)),
    }


@dataclass(frozen=True)
class Scenario:
    """One scenario's series, decisions, open trade and sizing."""

    bars: pd.DataFrame
    decisions: Decisions
    edge: str
    open_trade: dict[str, Any] | None = None
    holding_period_bars: int | None = None
    exit_params: dict[str, Any] = field(default_factory=dict)
    sizing: dict[str, Any] = field(default_factory=lambda: {"type": "fixed_quantity", "quantity": 1.0})
    sizing_point_value: float = 1.0


def _base_frame(generator: Any, n: int, *, seed: int = 20240609) -> pd.DataFrame:
    frame = generator(n=n, seed=seed)
    return frame.loc[:, list(_OHLC)].copy()


def _with_volatility(frame: pd.DataFrame, *, seed: int) -> pd.DataFrame:
    rng = np.random.default_rng(seed)
    out = frame.copy()
    volatility = 0.10 + rng.random(len(frame)) * 0.30
    volatility[13::29] = float("nan")
    volatility[19::31] = 0.0
    out["volatility"] = volatility
    return out


def scenarios(generator: Any) -> dict[str, Scenario]:
    """Every decision-step scenario, keyed by id."""
    bars = _base_frame(generator, 160)
    n = len(bars)
    seeded = scripted_decisions(n, seed=27_100)
    entry_price = float(bars.iloc[40]["close"])

    def long_trade() -> dict[str, Any]:
        return {"side": 1, "entry_price": entry_price, "entry_bar": 40, "off_grid_us": None}

    def short_trade() -> dict[str, Any]:
        return {"side": -1, "entry_price": entry_price, "entry_bar": 40, "off_grid_us": None}

    # d07's entry sits half an hour after bar 40 opened, so it is not a bar of the series and
    # `_timestamp_to_bar.get` finds nothing: the holding period can never close it.
    off_grid_us = timestamp_us(bars.index[40]) + 30 * 60 * 1_000_000

    return {
        "d01_no_trade_inverse_volatility": Scenario(
            bars=_with_volatility(bars, seed=27_101),
            decisions=seeded,
            edge="no open trade queues no exit, and volatility sizes each entry",
            sizing={
                "type": "inverse_volatility",
                "target_volatility_pct": 10.0,
                "max_contracts": 5,
                "min_contracts": 0,
            },
            sizing_point_value=0.2,
        ),
        "d02_long_open_scripted_exit": Scenario(
            bars=bars,
            decisions=seeded,
            open_trade=long_trade(),
            edge="the long exit column closes the open long",
        ),
        "d03_short_open_scripted_exit": Scenario(
            bars=bars,
            decisions=seeded,
            open_trade=short_trade(),
            edge="the short exit column closes the open short",
        ),
        "d04_trailing_atr_stop_long": Scenario(
            bars=bars,
            decisions=seeded,
            open_trade=long_trade(),
            exit_params={"trailing_stop_pct": 0.012, "stop_loss_atr": 1.5, "atr_period": 14},
            edge="trailing and ATR stop state carries across bars for a long",
        ),
        "d05_psar_time_stop_short": Scenario(
            bars=bars,
            decisions=seeded,
            open_trade=short_trade(),
            exit_params={
                "psar_af_start": 0.02,
                "psar_af_step": 0.02,
                "psar_af_max": 0.2,
                "max_bars_in_trade": 20,
            },
            edge="psar and the time stop carry their state for a short",
        ),
        "d06_holding_period_on_grid": Scenario(
            bars=bars,
            decisions=seeded.silent_exits(),
            open_trade=long_trade(),
            holding_period_bars=7,
            edge="an entry time on the bar grid closes at the holding period",
        ),
        "d07_holding_period_off_grid": Scenario(
            bars=bars,
            decisions=seeded.silent_exits(),
            open_trade={
                "side": 1,
                "entry_price": entry_price,
                "entry_bar": None,
                "off_grid_us": off_grid_us,
            },
            holding_period_bars=7,
            edge="an entry time off the bar grid is never closed by the holding period",
        ),
    }


def _make_trade(spec: dict[str, Any], index: pd.DatetimeIndex) -> Any:
    from q_backend.backtesting.models import OrderAction, Trade

    if spec["entry_bar"] is not None:
        entry_time = index[spec["entry_bar"]].to_pydatetime()
    else:
        entry_time = pd.Timestamp(spec["off_grid_us"], unit="us", tz="UTC").to_pydatetime()
    return Trade(
        id=TRADE_ID,
        order_id="golden-parity-order",
        symbol=SYMBOL,
        action=OrderAction.BUY if spec["side"] == 1 else OrderAction.SELL,
        quantity=1.0,
        entry_time=entry_time,
        entry_price=spec["entry_price"],
    )


def _identity(scenario_id: str, sizing: dict[str, Any]) -> Any:
    from q_backend.execution.domain import StrategyIdentity

    return StrategyIdentity(
        strategy_name="scripted",
        strategy_version=1,
        compiled_config={},
        config_hash=f"decision-step-{scenario_id}",
        symbol=SYMBOL,
        timeframe=TIMEFRAME,
        sizing_config=dict(sizing),
    )


def _strategy(scenario: Scenario) -> Any:
    return build_scripted_strategy(
        decisions=scenario.decisions,
        holding_period_bars=scenario.holding_period_bars,
        exit_params=scenario.exit_params,
    )


def run_scenario(scenario_id: str, scenario: Scenario) -> tuple[pd.DataFrame, dict[str, object], str]:
    """Run the reference and the evaluator, assert they agree, and encode the decisions."""
    from q_backend.execution.evaluator import StrategyEvaluator
    from q_backend.execution.parity import (
        augment_with_exit_columns,
        reference_queued_signals_by_close,
        signals_equal,
    )

    frame = augment_with_exit_columns(_strategy(scenario), scenario.bars)
    trade_spec = scenario.open_trade
    open_trade = _make_trade(trade_spec, scenario.bars.index) if trade_spec is not None else None

    reference = reference_queued_signals_by_close(
        _strategy(scenario),
        scenario.bars,
        timeframe=TIMEFRAME,
        open_trade=copy.deepcopy(open_trade) if open_trade is not None else None,
    )

    evaluator = StrategyEvaluator(
        deployment_id=f"decision-step-{scenario_id}",
        identity=_identity(scenario_id, scenario.sizing),
        initial_capital=INITIAL_CAPITAL,
        point_value=scenario.sizing_point_value,
        strategy=_strategy(scenario),
        window_bound=len(scenario.bars),
    )
    if open_trade is not None:
        evaluator.set_open_trade(copy.deepcopy(open_trade))
    results = evaluator.ingest_completed_bars(scenario.bars)

    if len(results) != len(scenario.bars):
        raise RuntimeError(f"{scenario_id}: evaluator returned {len(results)} of {len(scenario.bars)} bars")

    reference_by_close = {close: (exits, entries) for close, exits, entries in reference}
    requested_quantity: list[float] = []
    for result in results:
        if result.bar_close_time not in reference_by_close:
            raise RuntimeError(f"{scenario_id}: no reference row for close {result.bar_close_time}")
        ref_exits, ref_entries = reference_by_close[result.bar_close_time]
        fwd_exits, fwd_entries = _signals_from_result(result)
        if not signals_equal(ref_exits, fwd_exits):
            raise RuntimeError(f"{scenario_id}: exit-signal parity mismatch at {result.bar_close_time}")
        if not signals_equal(ref_entries, fwd_entries):
            raise RuntimeError(f"{scenario_id}: entry-signal parity mismatch at {result.bar_close_time}")
        requested_quantity.append(
            float("nan") if result.requested_quantity is None else float(result.requested_quantity)
        )

    expected = encode_decisions(reference, requested_quantity)
    queued = sum(len(exits) for _, exits, _ in reference)
    sized = int(np.count_nonzero(~np.isnan(np.asarray(requested_quantity, dtype=np.float64))))
    return frame, expected, f"bars={len(scenario.bars)} queued_exits={queued} sized={sized}"


def _signals_from_result(result: Any) -> tuple[list[Any], list[Any]]:
    from q_backend.backtesting.models import Signal

    exits = [Signal.model_validate(s) for s in result.queued_exit_signals]
    entries = [Signal.model_validate(s) for s in result.queued_entry_signals]
    return exits, entries


def encode_inputs(frame: pd.DataFrame, scenario: Scenario) -> dict[str, object]:
    """Every column the step reads, from the same augmented frame the evaluator saw."""
    inputs: dict[str, object] = {"time_us": encode_column(index_us(frame.index))}
    for column in _OHLC:
        if column in frame.columns:
            inputs[column] = encode_column(frame[column].to_numpy(dtype=np.float64))
    inputs[ENTRY_COLUMN] = encode_column(np.asarray(scenario.decisions.entry, dtype=np.int64))
    inputs[EXIT_LONG_COLUMN] = encode_column(np.asarray(scenario.decisions.exit_long, dtype=np.int64))
    inputs[EXIT_SHORT_COLUMN] = encode_column(np.asarray(scenario.decisions.exit_short, dtype=np.int64))
    inputs[STRENGTH_COLUMN] = encode_column(scenario.decisions.strength.astype(np.float64))
    if scenario.holding_period_bars is not None:
        inputs[BAR_INDEX_COLUMN] = encode_column(frame[BAR_INDEX_COLUMN].to_numpy(dtype=np.int64))
    if "volatility" in frame.columns:
        inputs["volatility"] = encode_column(frame["volatility"].to_numpy(dtype=np.float64))
    for column in frame.columns:
        if str(column).startswith("atr_") or str(column).startswith("donchian_"):
            inputs[str(column)] = encode_column(frame[column].to_numpy(dtype=np.float64))
    return inputs


class DecisionStepFamily:
    """Record section D's queued signals and the quantity the evaluator would request."""

    name: Final[str] = "decision_step"
    environment: Final[Literal["numeric", "backend"]] = "backend"
    policy: Final[dict[str, object]] = {"kind": "exact"}

    def export(self, source: BackendSource, out_dir: Path) -> list[str]:
        generator = load_generator(source, "tests/backtesting/test_goldens.py", "synthetic_ohlcv")
        specs = scenarios(generator)

        provenance = build_provenance(
            source,
            (
                "src/q_backend/execution/parity.py",
                "src/q_backend/execution/evaluator.py",
                "src/q_backend/execution/signal_eval.py",
                "src/q_backend/execution/indicator_frame.py",
                "src/q_backend/backtesting/exit_strategy.py",
                "src/q_backend/backtesting/position_sizing.py",
                "src/q_backend/backtesting/strategies/lai_lau_common.py",
                "tests/backtesting/test_goldens.py",
            ),
        )

        family_dir = out_dir / self.name
        family_dir.mkdir(parents=True, exist_ok=True)
        exported: list[str] = []

        for scenario_id in SCENARIO_IDS:
            scenario = specs[scenario_id]
            frame, expected, note = run_scenario(scenario_id, scenario)

            payload = {
                "format": FORMAT,
                "family": self.name,
                "fixture_id": scenario_id,
                "policy": dict(self.policy),
                "provenance": provenance,
                "cases": [
                    {
                        "case_id": scenario_id,
                        "edge": scenario.edge,
                        "config": {
                            "initial_capital": INITIAL_CAPITAL,
                            "timeframe": TIMEFRAME,
                            "holding_period_bars": scenario.holding_period_bars,
                            "exit_params": dict(scenario.exit_params),
                            "sizing": sizing_model(scenario.sizing).model_dump(),
                            "sizing_point_value": scenario.sizing_point_value,
                            "open_trade": _encode_open_trade(scenario.open_trade),
                        },
                        "inputs": encode_inputs(frame, scenario),
                        "expected": expected,
                    }
                ],
            }
            path = family_dir / f"{scenario_id}.json"
            path.write_text(dumps_fixture(payload), encoding="utf-8")
            exported.append(scenario_id)
            sys.stderr.write(f"exported decision_step/{scenario_id} {note}\n")

        return exported


def _encode_open_trade(spec: dict[str, Any] | None) -> dict[str, Any] | None:
    if spec is None:
        return None
    return {
        "id": TRADE_ID,
        "side": spec["side"],
        "entry_price": spec["entry_price"],
        "entry_bar": spec["entry_bar"],
    }
