"""Candle-engine reference family: `BacktestEngine._run_single_chunk` ledger parity.

Each scenario drives the pinned bar loop with a `ScriptedStrategy` over `synthetic_ohlcv`
and records the resulting trade ledger in open order, with every time mapped back to a bar
position. `_run_single_chunk` is the function the Rust kernel replaces: `force_close_at_end`
is a parameter there, so the day split stays in the caller and the fixture never depends on
pandas date grouping.
"""

from __future__ import annotations

import importlib.metadata
import platform
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
    git_blob_id,
    load_generator,
)
from families.exit_rules import RULE_CODE
from families.scripted_strategy import (
    BAR_INDEX_COLUMN,
    ENTRY_COLUMN,
    EXIT_LONG_COLUMN,
    EXIT_SHORT_COLUMN,
    STRENGTH_COLUMN,
    SYMBOL,
    Decisions,
    augment,
    build_scripted_strategy,
    scripted_decisions,
)

# Trade.exit_reason strings the engine writes, as q_engine::candle::ExitReason::code.
REASON_CODE: Final[dict[str, int]] = {
    **RULE_CODE,
    "SIGNAL": 11,
    "END_OF_DAY": 12,
    "FORCE_CLOSE": 13,
}
STILL_OPEN: Final = -1

SCENARIO_IDS: Final = (
    "k01_strategy_exits",
    "k02_stop_target_rules",
    "k03_atr_trailing_rules",
    "k04_long_short_both_open",
    "k05_rule_and_strategy_same_bar",
    "k06_pyramid_cap_trim",
    "k07_safety_margin_compounding",
    "k08_inverse_volatility",
    "k09_strength_floor_to_zero",
    "k10_costs",
    "k11_holding_period",
    "k12_day_trade_sequential",
    "k13_day_trade_chunk_force_close",
    "k14_trade_start_warmup",
    "k15_close_only_series",
    "k16_repeated_times",
    "k17_all_rules_psar_ratchet_breakeven",
)

_OHLC: Final = ("open", "high", "low", "close")
_US_PER_HOUR: Final = 3_600_000_000
DAY_TRADE_TIMES: Final = ("09:00", "16:00", "17:00")

# 15-minute bars: bars per day, and the positions of 09:00, 16:00 and 17:00 within one.
_BARS_PER_DAY: Final = 96
_ENTRY_START_BAR: Final = 36
_ENTRY_END_BAR: Final = 64
_FORCE_CLOSE_BAR: Final = 68


def index_us(index: pd.DatetimeIndex) -> np.ndarray:
    """Bar times as int64 wall-clock microseconds; reject sub-microsecond remainders."""
    if not isinstance(index, pd.DatetimeIndex):
        raise TypeError(f"expected DatetimeIndex, got {type(index)!r}")
    naive = index.tz_convert(None) if index.tz is not None else index
    ns = naive.as_unit("ns").asi8
    if np.any(ns % 1000 != 0):
        raise ValueError("timestamp has sub-microsecond remainder; cannot encode exactly as us")
    return (ns // 1000).astype(np.int64)


def timestamp_us(value: Any) -> int:
    """One `Trade` timestamp as int64 wall-clock microseconds."""
    ts = pd.Timestamp(value)
    if ts.tz is not None:
        ts = ts.tz_convert("UTC").tz_localize(None)
    ns = ts.as_unit("ns").value
    if ns % 1000 != 0:
        raise ValueError(f"timestamp {value!r} has a sub-microsecond remainder")
    return ns // 1000


def bars_by_time(times: np.ndarray) -> dict[int, list[int]]:
    """Map each bar time to every position that carries it."""
    positions: dict[int, list[int]] = {}
    for bar, value in enumerate(times):
        positions.setdefault(int(value), []).append(bar)
    return positions


def unique_bar(positions: dict[int, list[int]], value: Any, *, field_name: str, trade_id: str) -> int:
    """The single bar position for a trade time, or a failure naming the ambiguity.

    A repeated timestamp makes the position unrecoverable from the time alone. The exporter
    refuses to guess: a scenario that fills on a repeated bar must be rescripted.
    """
    key = timestamp_us(value)
    matches = positions.get(key, [])
    if len(matches) == 1:
        return matches[0]
    if not matches:
        raise ValueError(f"trade {trade_id}: {field_name} {value!r} is not a bar of the series")
    raise ValueError(
        f"trade {trade_id}: {field_name} {value!r} matches bars {matches}; "
        "the ledger cannot name a unique bar position"
    )


def _side_code(trade: Any) -> int:
    action = getattr(trade.action, "value", trade.action)
    if str(action) == "BUY":
        return 1
    if str(action) == "SELL":
        return -1
    raise ValueError(f"trade {trade.id}: unexpected action {action!r}")


def encode_ledger(trades: list[Any], index: pd.DatetimeIndex) -> dict[str, object]:
    """Encode the trade ledger in open order, with times as bar positions.

    Open trades carry `exit_bar = -1`, `exit_reason = -1` and NaN exit price and pnl, which
    is how `TradeLedger` leaves them.
    """
    positions = bars_by_time(index_us(index))
    entry_bar: list[int] = []
    exit_bar: list[int] = []
    side: list[int] = []
    quantity: list[float] = []
    entry_price: list[float] = []
    exit_price: list[float] = []
    commission: list[float] = []
    pnl: list[float] = []
    exit_reason: list[int] = []

    for trade in trades:
        entry_bar.append(unique_bar(positions, trade.entry_time, field_name="entry_time", trade_id=trade.id))
        side.append(_side_code(trade))
        quantity.append(float(trade.quantity))
        entry_price.append(float(trade.entry_price))
        commission.append(float(trade.commission))
        if trade.exit_time is None:
            exit_bar.append(STILL_OPEN)
            exit_price.append(float("nan"))
            pnl.append(float("nan"))
            exit_reason.append(STILL_OPEN)
            continue
        exit_bar.append(unique_bar(positions, trade.exit_time, field_name="exit_time", trade_id=trade.id))
        exit_price.append(float(trade.exit_price))
        pnl.append(float("nan") if trade.pnl is None else float(trade.pnl))
        reason = trade.exit_reason
        if reason not in REASON_CODE:
            raise ValueError(f"trade {trade.id}: unknown exit reason {reason!r}")
        exit_reason.append(REASON_CODE[reason])

    return {
        "entry_bar": encode_column(np.asarray(entry_bar, dtype=np.int64)),
        "exit_bar": encode_column(np.asarray(exit_bar, dtype=np.int64)),
        "side": encode_column(np.asarray(side, dtype=np.int64)),
        "quantity": encode_column(np.asarray(quantity, dtype=np.float64)),
        "entry_price": encode_column(np.asarray(entry_price, dtype=np.float64)),
        "exit_price": encode_column(np.asarray(exit_price, dtype=np.float64)),
        "commission": encode_column(np.asarray(commission, dtype=np.float64)),
        "pnl": encode_column(np.asarray(pnl, dtype=np.float64)),
        "exit_reason": encode_column(np.asarray(exit_reason, dtype=np.int64)),
    }


@dataclass(frozen=True)
class Scenario:
    """One scenario's frame, decisions and engine configuration."""

    bars: pd.DataFrame
    decisions: Decisions
    edge: str
    initial_capital: float = 100_000.0
    point_value: float = 1.0
    sizing: dict[str, Any] = field(default_factory=lambda: {"type": "fixed_quantity", "quantity": 1.0})
    sizing_point_value: float = 1.0
    costs: tuple[float, float] | None = None
    holding_period_bars: int | None = None
    exit_params: dict[str, Any] = field(default_factory=dict)
    day_trade: bool = False
    force_close_at_end: bool = False
    trade_start_bar: int | None = None


def _base_frame(
    generator: Any,
    n: int,
    *,
    seed: int = 20240609,
    freq: str = "h",
) -> pd.DataFrame:
    frame = generator(n=n, seed=seed, freq=freq)
    return frame.loc[:, list(_OHLC)].copy()


def _volatility_column(n: int, *, seed: int) -> np.ndarray:
    """An annualised-volatility column with the bars the sizer must refuse."""
    rng = np.random.default_rng(seed)
    volatility = 0.10 + rng.random(n) * 0.30
    volatility[11::37] = float("nan")
    volatility[17::41] = 0.0
    volatility[23::43] = -0.05
    return volatility


def _repeat_time(frame: pd.DataFrame, bar: int) -> pd.DataFrame:
    """Give `bar` the timestamp of `bar - 1`, so two bars share one time."""
    times = list(frame.index)
    times[bar] = times[bar - 1]
    out = frame.copy()
    out.index = pd.DatetimeIndex(times, name=frame.index.name)
    return out


def scenarios(generator: Any) -> dict[str, Scenario]:
    """Every scenario, keyed by id, over fixed-seed series and hand-edited decisions."""
    hourly = _base_frame(generator, 240)
    n = len(hourly)

    seeded = scripted_decisions(n, seed=27_001)

    # k04: one hand-built window where a long and a short are open together.
    both_open = (
        seeded.without(range(28, 48))
        .with_entries([30], side=1, strength=0.5)
        .with_entries([33], side=-1, strength=0.5)
    )
    both_open = Decisions(
        both_open.entry,
        _mask_at(both_open.exit_long, [40]),
        both_open.exit_short,
        both_open.strength,
    )

    # k06: four consecutive entries against a cap of 2.5, so one is trimmed and two skipped.
    pyramid = (
        seeded.without(range(38, 52))
        .with_entries([40, 41, 42, 43], side=1, strength=1.0)
    )

    # k09: quantity 1 scaled by strength, so every strength below 1.0 floors to nothing.
    floor_to_zero = scripted_decisions(n, seed=27_009, entry_rate=0.12, exit_rate=0.10)

    day_frame = _base_frame(generator, 232, freq="15min")
    day_decisions = _day_trade_decisions(len(day_frame), seed=27_012)

    single_day = _base_frame(generator, _BARS_PER_DAY, freq="15min")
    chunk_close = (
        scripted_decisions(len(single_day), seed=27_013)
        .without(range(86, len(single_day)))
        .with_entries([90], side=1, strength=1.0)
    )

    close_only = hourly.loc[:, ["close"]].copy()

    repeated = _repeat_time(hourly, 120)
    repeated_decisions = seeded.without(range(117, 123))

    return {
        "k01_strategy_exits": Scenario(
            bars=hourly,
            decisions=seeded,
            edge="scripted long and short exits under a fixed-quantity cap",
        ),
        "k02_stop_target_rules": Scenario(
            bars=hourly,
            decisions=seeded,
            exit_params={"stop_loss_pct": 0.015, "take_profit_pct": 0.010},
            edge="fixed_sl and fixed_tp both close trades",
        ),
        "k03_atr_trailing_rules": Scenario(
            bars=hourly,
            decisions=seeded,
            # The chandelier distance is 0.7 ATR and the trailing distance 1.5% of the peak,
            # which cross each other over the series, so both trailing rules get to win.
            exit_params={
                "stop_loss_atr": 1.0,
                "atr_period": 14,
                "trailing_stop_pct": 0.015,
                "chandelier_atr_mult": 0.7,
            },
            edge="atr_sl, trailing and chandelier close trades from indicator columns",
        ),
        "k04_long_short_both_open": Scenario(
            bars=hourly,
            decisions=both_open,
            sizing={"type": "fixed_quantity", "quantity": 2.0, "scale_by_signal_strength": True},
            edge="a short opens while a long is open and one exit closes both",
        ),
        "k05_rule_and_strategy_same_bar": Scenario(
            bars=hourly,
            decisions=_all_exits(seeded),
            exit_params={"stop_loss_pct": 0.010},
            edge="a rule exit suppresses the scripted exit on the same bar",
        ),
        "k06_pyramid_cap_trim": Scenario(
            bars=hourly,
            decisions=pyramid,
            sizing={"type": "fixed_quantity", "quantity": 2.5, "scale_by_signal_strength": True},
            edge="the cap trims one entry to a fraction and skips the rest",
        ),
        "k07_safety_margin_compounding": Scenario(
            bars=hourly,
            decisions=seeded,
            sizing={
                "type": "fixed_safety_margin",
                "safety_margin_per_contract": 5_000.0,
                "min_contracts": 1,
                "max_contracts": 30,
            },
            edge="realised pnl changes the next entry's contract count",
        ),
        "k08_inverse_volatility": Scenario(
            bars=_with_volatility(hourly, seed=27_008),
            decisions=seeded,
            sizing={
                "type": "inverse_volatility",
                "target_volatility_pct": 10.0,
                "max_contracts": 5,
                "min_contracts": 1,
            },
            # The sizer's point value is 200 while the trade's is 1, so the contract count
            # lands inside the 1..5 band instead of always clamping to the maximum.
            sizing_point_value=200.0,
            edge="volatility sets the contract count, and NaN, zero and negative bars size nothing",
        ),
        "k09_strength_floor_to_zero": Scenario(
            bars=hourly,
            decisions=floor_to_zero,
            sizing={"type": "fixed_quantity", "quantity": 1.0, "scale_by_signal_strength": True},
            edge="a strength below 1.0 floors the quantity to zero and opens nothing",
        ),
        "k10_costs": Scenario(
            bars=hourly,
            decisions=seeded,
            point_value=0.2,
            costs=(1.5, 3.0),
            edge="both sides of every trade are charged per contract and in bps",
        ),
        "k11_holding_period": Scenario(
            bars=hourly,
            decisions=seeded.silent_exits(),
            holding_period_bars=7,
            edge="the holding period closes each trade eight bars after its entry",
        ),
        "k12_day_trade_sequential": Scenario(
            bars=day_frame,
            decisions=day_decisions,
            day_trade=True,
            edge="the force-close time and the last bar of the day both close at END_OF_DAY",
        ),
        "k13_day_trade_chunk_force_close": Scenario(
            bars=single_day,
            decisions=chunk_close,
            force_close_at_end=True,
            edge="the end of the chunk closes the open trade at FORCE_CLOSE",
        ),
        "k14_trade_start_warmup": Scenario(
            bars=hourly,
            decisions=seeded,
            trade_start_bar=50,
            edge="bars before trade start take no action at all",
        ),
        "k15_close_only_series": Scenario(
            bars=close_only,
            decisions=seeded,
            exit_params={"stop_loss_pct": 0.020, "trailing_stop_pct": 0.015},
            edge="a series with only a close column fills and evaluates at the close",
        ),
        "k16_repeated_times": Scenario(
            bars=repeated,
            decisions=repeated_decisions,
            edge="two bars share a timestamp and the loop still walks positions",
        ),
        "k17_all_rules_psar_ratchet_breakeven": Scenario(
            bars=hourly,
            decisions=seeded,
            # psar is deliberately absent. Its SAR seeds at `min(entry_price, low)` on the
            # entry bar and `should_exit` compares that bar's own low against it, so it fires
            # on the first bar of every trade and no later rule can ever win. Enabling it here
            # would leave the ratchet, the time stop and the donchian stop unpinned; psar's
            # own reason is pinned by `decision_step/d05`, where it is the rule that wins.
            exit_params={
                "breakeven_trigger_pct": 0.03,
                "breakeven_offset_pct": 0.001,
                "target_ratchet_atr": 1.5,
                "atr_period": 14,
                "max_bars_in_trade": 8,
                "donchian_exit_period": 20,
            },
            edge="breakeven, the ratchet, the time stop and the donchian stop all close trades",
        ),
    }


def _mask_at(mask: np.ndarray, bars: list[int]) -> np.ndarray:
    out = mask.copy()
    for bar in bars:
        out[bar] = True
    return out


def _all_exits(decisions: Decisions) -> Decisions:
    """Both exit columns true on every bar, so every rule exit suppresses a scripted one."""
    return Decisions(
        decisions.entry.copy(),
        np.ones(len(decisions), dtype=bool),
        np.ones(len(decisions), dtype=bool),
        decisions.strength.copy(),
    )


def _with_volatility(frame: pd.DataFrame, *, seed: int) -> pd.DataFrame:
    out = frame.copy()
    out["volatility"] = _volatility_column(len(frame), seed=seed)
    return out


def _day_trade_decisions(n: int, *, seed: int) -> Decisions:
    """Entries inside and outside the entry window, and trades that survive to each close."""
    decisions = scripted_decisions(n, seed=seed, entry_rate=0.04, exit_rate=0.05)
    # Day 1: an entry that survives the 17:00 force close, entered at the window's last bar.
    decisions = decisions.without(range(_ENTRY_END_BAR - 2, _FORCE_CLOSE_BAR + 2))
    decisions = decisions.with_entries([_ENTRY_END_BAR], side=1, strength=1.0)
    # Day 2: an entry outside the window (08:00) that must be dropped, then one inside it.
    day2 = _BARS_PER_DAY
    decisions = decisions.without(range(day2 + 28, day2 + _FORCE_CLOSE_BAR))
    decisions = decisions.with_entries([day2 + 32], side=-1, strength=1.0)
    decisions = decisions.with_entries([day2 + 40], side=-1, strength=1.0)
    # Day 3 is a partial day ending before 17:00, so its last bar is the section-E close.
    day3 = 2 * _BARS_PER_DAY
    decisions = decisions.without(range(day3 + _ENTRY_START_BAR - 2, n))
    decisions = decisions.with_entries([day3 + _ENTRY_START_BAR], side=1, strength=1.0)
    return decisions


def sizing_model(sizing: dict[str, Any]) -> Any:
    """Validate a sizing mapping into the backend's discriminated `PositionSizingConfig`."""
    from pydantic import TypeAdapter
    from q_backend.backtesting.position_sizing import PositionSizingConfig

    return TypeAdapter(PositionSizingConfig).validate_python(sizing)


def build_sizer(sizing: dict[str, Any], point_value: float) -> tuple[Any, dict[str, Any]]:
    """The sizer the engine will use, and the fully-defaulted mapping to store."""
    from q_backend.backtesting.position_sizing import build_position_sizer

    model = sizing_model(sizing)
    return build_position_sizer(model, point_value=point_value), model.model_dump()


def run_scenario(scenario: Scenario) -> tuple[pd.DataFrame, list[Any], dict[str, Any]]:
    """Run one scenario and return the augmented frame, the ledger, and the stored config."""
    from q_backend.backtesting.costs import TransactionCostConfig
    from q_backend.backtesting.engine import BacktestEngine

    strategy = build_scripted_strategy(
        decisions=scenario.decisions,
        holding_period_bars=scenario.holding_period_bars,
        exit_params=scenario.exit_params,
    )
    frame = augment(strategy, scenario.bars)
    sizer, sizing_dump = build_sizer(scenario.sizing, scenario.sizing_point_value)
    costs = (
        TransactionCostConfig(cost_per_contract=scenario.costs[0], cost_bps=scenario.costs[1])
        if scenario.costs is not None
        else None
    )
    engine = BacktestEngine(
        strategy=strategy,
        sizer=sizer,
        initial_capital=scenario.initial_capital,
        point_values={SYMBOL: scenario.point_value},
        day_trade=scenario.day_trade,
        day_trade_start_time=DAY_TRADE_TIMES[0],
        day_trade_end_time=DAY_TRADE_TIMES[1],
        day_trade_close_time=DAY_TRADE_TIMES[2],
        costs=costs,
    )
    trade_start = (
        frame.index[scenario.trade_start_bar].to_pydatetime()
        if scenario.trade_start_bar is not None
        else None
    )
    registry = engine._run_single_chunk(
        frame,
        force_close_at_end=scenario.force_close_at_end,
        trade_start=trade_start,
    )

    config: dict[str, Any] = {
        "initial_capital": scenario.initial_capital,
        "point_value": scenario.point_value,
        "costs": (
            None
            if scenario.costs is None
            else {"per_contract": scenario.costs[0], "bps": scenario.costs[1]}
        ),
        "sizing": sizing_dump,
        "sizing_point_value": scenario.sizing_point_value,
        "holding_period_bars": scenario.holding_period_bars,
        "exit_params": dict(scenario.exit_params),
        "day_trade_us": _day_trade_us(scenario.day_trade),
        "force_close_at_end": scenario.force_close_at_end,
        "trade_start_bar": scenario.trade_start_bar,
    }
    return frame, registry.get_all_trades(), config


def _day_trade_us(day_trade: bool) -> list[int] | None:
    if not day_trade:
        return None
    return [
        9 * _US_PER_HOUR,
        16 * _US_PER_HOUR,
        17 * _US_PER_HOUR,
    ]


def encode_inputs(frame: pd.DataFrame, scenario: Scenario) -> dict[str, object]:
    """Every column the kernel reads, including the tradable mask and indicator columns."""
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
    if scenario.trade_start_bar is not None:
        tradable = np.zeros(len(frame), dtype=np.int64)
        tradable[scenario.trade_start_bar :] = 1
        inputs["tradable"] = encode_column(tradable)
    for column in frame.columns:
        if str(column).startswith("atr_") or str(column).startswith("donchian_"):
            inputs[str(column)] = encode_column(frame[column].to_numpy(dtype=np.float64))
    return inputs


def _reason_codes(ledger: dict[str, object]) -> list[int]:
    column = ledger["exit_reason"]
    assert isinstance(column, dict)
    values = column["values"]
    assert isinstance(values, list)
    return [int(v) for v in values]


def _int_column(ledger: dict[str, object], name: str) -> list[int]:
    column = ledger[name]
    assert isinstance(column, dict)
    values = column["values"]
    assert isinstance(values, list)
    return [int(v) for v in values]


def _float_column(ledger: dict[str, object], name: str) -> list[float]:
    column = ledger[name]
    assert isinstance(column, dict)
    values = column["values"]
    assert isinstance(values, list)
    return [float("nan") if v is None else float(v) for v in values]


def check_edge(scenario_id: str, scenario: Scenario, frame: pd.DataFrame, ledger: dict[str, object]) -> str:
    """Fail unless the scenario opened a trade and its ledger shows the edge it names."""
    entry_bar = _int_column(ledger, "entry_bar")
    exit_bar = _int_column(ledger, "exit_bar")
    side = _int_column(ledger, "side")
    quantity = _float_column(ledger, "quantity")
    reasons = _reason_codes(ledger)
    if not entry_bar:
        raise RuntimeError(f"{scenario_id}: opened no trades; rescript before committing")

    entries_scripted = int(np.count_nonzero(scenario.decisions.entry))
    note = f"trades={len(entry_bar)}"

    def require(condition: bool, detail: str) -> None:
        if not condition:
            raise RuntimeError(f"{scenario_id}: {detail} (edge: {scenario.edge})")

    if scenario_id == "k01_strategy_exits":
        require(1 in side and -1 in side, "no long and short pair")
        require(REASON_CODE["SIGNAL"] in reasons, "no scripted exit fired")
    elif scenario_id == "k02_stop_target_rules":
        require(RULE_CODE["fixed_sl"] in reasons, "fixed_sl never fired")
        require(RULE_CODE["fixed_tp"] in reasons, "fixed_tp never fired")
    elif scenario_id == "k03_atr_trailing_rules":
        wanted = {RULE_CODE[name] for name in ("atr_sl", "trailing", "chandelier")}
        fired = wanted & set(reasons)
        require(fired == wanted, f"only {sorted(fired)} of {sorted(wanted)} indicator rules fired")
    elif scenario_id == "k04_long_short_both_open":
        require(_has_opposite_overlap(entry_bar, exit_bar, side), "no long and short were open together")
        require(REASON_CODE["SIGNAL"] in reasons, "no scripted exit closed the pair")
    elif scenario_id == "k05_rule_and_strategy_same_bar":
        rule_codes = {code for code in reasons if 0 <= code <= 10}
        require(bool(rule_codes), "no rule exit fired while both exit columns were true")
        require(REASON_CODE["SIGNAL"] in reasons, "no scripted exit fired")
    elif scenario_id == "k06_pyramid_cap_trim":
        require(any(0.0 < q < 1.0 for q in quantity), "no entry was trimmed to a fraction")
        require(len(entry_bar) < entries_scripted, "no entry was skipped at a full cap")
    elif scenario_id == "k07_safety_margin_compounding":
        require(len(set(quantity)) > 1, "every entry sized the same contract count")
    elif scenario_id == "k08_inverse_volatility":
        refused = _invalid_volatility_fills(frame, scenario)
        require(bool(refused), "no scripted entry landed on an unusable volatility bar")
        require(not set(refused) & set(entry_bar), "an unusable volatility bar still opened a trade")
        require(len(set(quantity)) > 1, "every entry clamped to the same contract count")
    elif scenario_id == "k09_strength_floor_to_zero":
        require(len(entry_bar) < entries_scripted, "no entry floored to zero")
        require(all(q == 1.0 for q in quantity), "a scaled quantity survived below 1.0")
    elif scenario_id == "k10_costs":
        require(all(c > 0.0 for c in _float_column(ledger, "commission")), "a trade was charged nothing")
    elif scenario_id == "k11_holding_period":
        spans = {b - a for a, b in zip(entry_bar, exit_bar) if b >= 0}
        require(spans == {8}, f"holding-period spans were {sorted(spans)}, not eight bars")
    elif scenario_id == "k12_day_trade_sequential":
        times = index_us(frame.index)
        after_close = [
            bar for bar in exit_bar if bar >= 0 and times[bar] % 86_400_000_000 >= 17 * _US_PER_HOUR
        ]
        require(bool(after_close), "nothing closed at the force-close time")
        require(len(frame) - 1 in exit_bar, "nothing closed on the chunk's last bar of the day")
        require(REASON_CODE["END_OF_DAY"] in reasons, "no close carried END_OF_DAY")
        require(_entry_dropped_outside_window(frame, scenario, entry_bar), "no entry was dropped outside the window")
    elif scenario_id == "k13_day_trade_chunk_force_close":
        require(REASON_CODE["FORCE_CLOSE"] in reasons, "the chunk's end closed nothing")
    elif scenario_id == "k14_trade_start_warmup":
        assert scenario.trade_start_bar is not None
        require(min(entry_bar) > scenario.trade_start_bar, "a trade filled at or before trade start")
        require(
            int(np.count_nonzero(scenario.decisions.entry[: scenario.trade_start_bar])) > 0,
            "no entry was scripted inside the warm-up, so the mask proved nothing",
        )
    elif scenario_id == "k15_close_only_series":
        require("open" not in frame.columns, "the frame still carries an open column")
        closes = frame["close"].to_numpy(dtype=np.float64)
        require(
            all(price == closes[bar] for bar, price in zip(entry_bar, _float_column(ledger, "entry_price"))),
            "an entry did not fill at its bar's close",
        )
    elif scenario_id == "k16_repeated_times":
        times = index_us(frame.index)
        repeats = [bar for bar, count in bars_by_time(times).items() if len(count) > 1]
        require(bool(repeats), "no timestamp is repeated")
    elif scenario_id == "k17_all_rules_psar_ratchet_breakeven":
        wanted = {
            RULE_CODE[name]
            for name in ("breakeven", "profit_target_ratchet", "time_stop", "donchian_stop")
        }
        fired = wanted & set(reasons)
        require(fired == wanted, f"only {sorted(fired)} of {sorted(wanted)} specialised rules fired")
    else:
        raise RuntimeError(f"no edge check for {scenario_id}")

    fired = sorted({code for code in reasons if code >= 0})
    return f"{note} reasons={fired}"


def _has_opposite_overlap(entry_bar: list[int], exit_bar: list[int], side: list[int]) -> bool:
    spans = [
        (entry, exit_ if exit_ >= 0 else 1 << 40, s)
        for entry, exit_, s in zip(entry_bar, exit_bar, side)
    ]
    for i, (a_start, a_end, a_side) in enumerate(spans):
        for b_start, b_end, b_side in spans[i + 1 :]:
            if a_side == b_side:
                continue
            if max(a_start, b_start) <= min(a_end, b_end):
                return True
    return False


def _invalid_volatility_fills(frame: pd.DataFrame, scenario: Scenario) -> list[int]:
    """Fill bars of scripted entries whose volatility cell the sizer must refuse."""
    volatility = frame["volatility"].to_numpy(dtype=np.float64)
    refused: list[int] = []
    for bar, entry in enumerate(scenario.decisions.entry[:-1]):
        if entry == 0:
            continue
        fill = bar + 1
        value = volatility[fill]
        if np.isnan(value) or value <= 0.0:
            refused.append(fill)
    return refused


def _entry_dropped_outside_window(frame: pd.DataFrame, scenario: Scenario, entry_bar: list[int]) -> bool:
    """True when a scripted entry outside the day's entry window never filled."""
    times = index_us(frame.index)
    filled = set(entry_bar)
    for bar, entry in enumerate(scenario.decisions.entry[:-1]):
        if entry == 0:
            continue
        time_of_day = times[bar] % 86_400_000_000
        outside = time_of_day < 9 * _US_PER_HOUR or time_of_day > 16 * _US_PER_HOUR
        if outside and (bar + 1) not in filled:
            return True
    return False


class CandleEngineFamily:
    """Drive `BacktestEngine._run_single_chunk` with scripted decisions and record the ledger."""

    name: Final[str] = "candle_engine"
    environment: Final[Literal["numeric", "backend"]] = "backend"
    policy: Final[dict[str, object]] = {"kind": "exact"}

    def export(self, source: BackendSource, out_dir: Path) -> list[str]:
        generator = load_generator(source, "tests/backtesting/test_goldens.py", "synthetic_ohlcv")
        specs = scenarios(generator)

        provenance = build_provenance(
            source,
            (
                "src/q_backend/backtesting/engine.py",
                "src/q_backend/backtesting/registry.py",
                "src/q_backend/backtesting/costs.py",
                "src/q_backend/backtesting/position_sizing.py",
                "src/q_backend/backtesting/exit_strategy.py",
                "src/q_backend/execution/indicator_frame.py",
                "src/q_backend/backtesting/strategies/lai_lau_common.py",
                "tests/backtesting/test_goldens.py",
            ),
        )

        family_dir = out_dir / self.name
        family_dir.mkdir(parents=True, exist_ok=True)
        exported: list[str] = []

        for scenario_id in SCENARIO_IDS:
            scenario = specs[scenario_id]
            frame, trades, config = run_scenario(scenario)
            ledger = encode_ledger(trades, frame.index)
            note = check_edge(scenario_id, scenario, frame, ledger)

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
                        "config": config,
                        "inputs": encode_inputs(frame, scenario),
                        "expected": ledger,
                    }
                ],
            }
            path = family_dir / f"{scenario_id}.json"
            path.write_text(dumps_fixture(payload), encoding="utf-8")
            exported.append(scenario_id)
            sys.stderr.write(f"exported candle_engine/{scenario_id} {note}\n")

        return exported


def build_provenance(source: BackendSource, paths: tuple[str, ...]) -> dict[str, object]:
    """The shared provenance block: repo, revision, environment and source blob ids."""
    return {
        "backend_repo": source.repo_url,
        "backend_rev": source.rev,
        "exporter": "tools/reference/export_reference.py",
        "environment": "backend",
        "python": platform.python_version(),
        "numpy": importlib.metadata.version("numpy"),
        "pandas": importlib.metadata.version("pandas"),
        "sources": [{"path": path, "blob": git_blob_id(source, path)} for path in paths],
    }
