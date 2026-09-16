"""A `TradingStrategy` whose decisions are read from columns, for the candle families.

The pin predates Q-024, so the backend's strategies still decide per row. This module
scripts those row decisions from arrays so the engine loop and the evaluator can be driven
over an arbitrary decision sequence: exactly the decision Q-024 moves into columns.

Everything that touches `q_backend` is built lazily, because `export_reference` imports the
family modules in the minimal numeric environment too.
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Any, Final

import numpy as np
import pandas as pd

SYMBOL: Final = "TEST"

ENTRY_COLUMN: Final = "entry_signal"
EXIT_LONG_COLUMN: Final = "exit_long_signal"
EXIT_SHORT_COLUMN: Final = "exit_short_signal"
STRENGTH_COLUMN: Final = "signal_strength"
BAR_INDEX_COLUMN: Final = "bar_index"

STRENGTH_CHOICES: Final = (0.25, 0.5, 0.75, 1.0)


@dataclass(frozen=True)
class Decisions:
    """One series' scripted decision columns, in Q-024's dtypes."""

    entry: np.ndarray  # int8, -1 / 0 / +1
    exit_long: np.ndarray  # bool
    exit_short: np.ndarray  # bool
    strength: np.ndarray  # float64, in [0, 1]

    def __len__(self) -> int:
        return len(self.entry)

    def with_entries(self, bars: list[int], side: int, strength: float) -> "Decisions":
        """Hand-edit: script `side` with `strength` at `bars`."""
        entry = self.entry.copy()
        strengths = self.strength.copy()
        for bar in bars:
            entry[bar] = side
            strengths[bar] = strength
        return Decisions(entry, self.exit_long.copy(), self.exit_short.copy(), strengths)

    def without(self, bars: range | list[int]) -> "Decisions":
        """Hand-edit: silence every decision at `bars`."""
        entry = self.entry.copy()
        exit_long = self.exit_long.copy()
        exit_short = self.exit_short.copy()
        strength = self.strength.copy()
        for bar in bars:
            entry[bar] = 0
            exit_long[bar] = False
            exit_short[bar] = False
            strength[bar] = 0.0
        return Decisions(entry, exit_long, exit_short, strength)

    def silent_exits(self) -> "Decisions":
        """Hand-edit: drop both exit columns, as a declared holding period requires."""
        return Decisions(
            self.entry.copy(),
            np.zeros(len(self), dtype=bool),
            np.zeros(len(self), dtype=bool),
            self.strength.copy(),
        )


def entry_column(buy: np.ndarray, sell: np.ndarray) -> np.ndarray:
    """Fold two scripted masks into Q-024's int8 entry column; a bar scripted both enters long.

    Q-024 writes the short side first and the long side over it, so a long wins wherever
    both fire. `entry_action` then turns that into one BUY.
    """
    buy_mask = np.asarray(buy, dtype=bool)
    sell_mask = np.asarray(sell, dtype=bool)
    if buy_mask.shape != sell_mask.shape:
        raise ValueError(f"buy and sell masks differ in length: {buy_mask.shape} vs {sell_mask.shape}")
    entry = np.zeros(len(buy_mask), dtype=np.int8)
    entry[sell_mask] = -1
    entry[buy_mask] = 1
    return entry


def entry_action(value: Any) -> str | None:
    """`BUY`, `SELL` or nothing for one bar's entry cell."""
    if value is None or (isinstance(value, float) and np.isnan(value)):
        return None
    code = int(value)
    if code == 1:
        return "BUY"
    if code == -1:
        return "SELL"
    return None


def scripted_decisions(
    n: int,
    *,
    seed: int,
    entry_rate: float = 0.08,
    exit_rate: float = 0.12,
) -> Decisions:
    """Fixed-seed decisions: independent long/short entry and exit masks plus strengths."""
    rng = np.random.default_rng(seed)
    buy = rng.random(n) < entry_rate
    sell = rng.random(n) < entry_rate
    exit_long = rng.random(n) < exit_rate
    exit_short = rng.random(n) < exit_rate
    entry = entry_column(buy, sell)
    strength = np.zeros(n, dtype=np.float64)
    drawn = rng.choice(np.asarray(STRENGTH_CHOICES, dtype=np.float64), size=n)
    strength[entry != 0] = drawn[entry != 0]
    return Decisions(entry, exit_long, exit_short, strength)


def attach_decisions(frame: pd.DataFrame, decisions: Decisions, *, with_bar_index: bool) -> pd.DataFrame:
    """Join the decision columns to `frame` by position, never by timestamp.

    Position is the only join that survives a repeated timestamp, and it is what Q-024's
    columns are indexed by.
    """
    if len(frame) != len(decisions):
        raise ValueError(f"frame has {len(frame)} bars but decisions have {len(decisions)}")
    out = frame.copy()
    out[ENTRY_COLUMN] = decisions.entry
    out[EXIT_LONG_COLUMN] = decisions.exit_long
    out[EXIT_SHORT_COLUMN] = decisions.exit_short
    out[STRENGTH_COLUMN] = decisions.strength
    if with_bar_index:
        out[BAR_INDEX_COLUMN] = np.arange(len(out), dtype=np.int64)
    return out


_CLASS_CACHE: list[type] = []


def _scripted_strategy_class() -> type:
    """Define `ScriptedStrategy` against the backend's base class on first use."""
    if _CLASS_CACHE:
        return _CLASS_CACHE[0]

    from q_backend.backtesting.models import OrderAction, Signal, SignalAction
    from q_backend.backtesting.strategies.lai_lau_common import (
        build_timestamp_to_bar,
        fixed_holding_period_exits,
    )
    from q_backend.backtesting.strategy import TradingStrategy

    class ScriptedStrategy(TradingStrategy):
        """Reads each bar's entry, exit and strength from the scripted columns."""

        def __init__(
            self,
            *,
            symbol: str,
            decisions: Decisions,
            holding_period_bars: int | None = None,
            **exit_params: Any,
        ) -> None:
            super().__init__(**exit_params)
            self.symbol = symbol
            self.decisions = decisions
            self.holding_period_bars = holding_period_bars
            self._timestamp_to_bar: pd.Series | None = None

        def compute_indicators(self, data: pd.DataFrame) -> pd.DataFrame:
            frame = attach_decisions(
                data,
                self.decisions,
                with_bar_index=self.holding_period_bars is not None,
            )
            if self.holding_period_bars is not None:
                self._timestamp_to_bar = build_timestamp_to_bar(frame)
            return frame

        def check_entry_conditions(self, current_data: pd.Series) -> list[Any]:
            action = entry_action(current_data.get(ENTRY_COLUMN, 0))
            if action is None:
                return []
            strength = float(current_data.get(STRENGTH_COLUMN, 0.0))
            return [
                Signal(
                    symbol=self.symbol,
                    action=SignalAction.BUY if action == "BUY" else SignalAction.SELL,
                    strength=strength,
                )
            ]

        def check_exit_conditions(self, current_data: pd.Series, open_trades: list[Any]) -> list[Any]:
            if not open_trades:
                return []
            if self.holding_period_bars is not None:
                if self._timestamp_to_bar is None:
                    raise RuntimeError("compute_indicators must run before a holding-period exit check")
                return fixed_holding_period_exits(
                    current_data,
                    open_trades,
                    self.symbol,
                    self.holding_period_bars,
                    self._timestamp_to_bar,
                )
            exit_long = bool(current_data.get(EXIT_LONG_COLUMN, False))
            exit_short = bool(current_data.get(EXIT_SHORT_COLUMN, False))
            signals: list[Any] = []
            # Registry order, as the engine's `get_open_trades` yields it.
            for trade in open_trades:
                if trade.symbol != self.symbol:
                    continue
                fires = exit_long if trade.action == OrderAction.BUY else exit_short
                if fires:
                    signals.append(Signal(symbol=self.symbol, action=SignalAction.CLOSE))
            return signals

        def get_chart_indicators(self) -> list[Any]:
            return []

    _CLASS_CACHE.append(ScriptedStrategy)
    return ScriptedStrategy


def build_scripted_strategy(
    *,
    decisions: Decisions,
    symbol: str = SYMBOL,
    holding_period_bars: int | None = None,
    exit_params: dict[str, Any] | None = None,
) -> Any:
    """One fresh scripted strategy, with its own exit-rule state."""
    cls = _scripted_strategy_class()
    return cls(
        symbol=symbol,
        decisions=decisions,
        holding_period_bars=holding_period_bars,
        **(exit_params or {}),
    )


def augment(strategy: Any, frame: pd.DataFrame) -> pd.DataFrame:
    """The frame the engine's bar loop will read: indicators plus exit-rule columns.

    Pre-augmenting means the exporter can encode the exact indicator columns the run used;
    the engine's own completion then finds them present and does nothing.
    """
    from q_backend.execution.indicator_frame import augment_indicator_frame

    return augment_indicator_frame(strategy, frame)
