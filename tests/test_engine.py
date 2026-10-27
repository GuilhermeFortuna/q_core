"""Wheel-level tests for q_core.engine candle kernel projection."""

from __future__ import annotations

import subprocess
import sys
import tempfile
from pathlib import Path


def main() -> None:
    root = Path(__file__).parents[1]
    dist = root / "dist"
    wheels = list(dist.glob("*.whl"))
    if not wheels:
        raise RuntimeError(f"No wheels found in {dist}. Run `make wheel` first.")
    wheel_path = sorted(wheels)[-1]

    with tempfile.TemporaryDirectory() as tmpdir:
        venv_dir = Path(tmpdir) / "venv"
        subprocess.check_call(["uv", "venv", str(venv_dir)], stdout=subprocess.DEVNULL)
        python_bin = venv_dir / "bin" / "python"
        subprocess.check_call(
            [
                "uv",
                "pip",
                "install",
                "--python",
                str(python_bin),
                str(wheel_path),
                "numpy",
            ],
            stdout=subprocess.DEVNULL,
        )
        res = subprocess.run(
            [str(python_bin), "-c", CASES],
            capture_output=True,
            text=True,
            cwd=str(root),
        )
        if res.returncode != 0:
            sys.stderr.write(res.stdout)
            sys.stderr.write(res.stderr)
            sys.exit(res.returncode)
        print(res.stdout.strip())
        print("Engine wheel tests passed successfully.")


CASES = r"""
import json
from pathlib import Path
import shutil
import tempfile

import numpy as np
import q_core.engine as engine

ROOT = Path.cwd()
FIXTURES = ROOT / "fixtures" / "reference"
ENTRY = "entry_signal"
EXIT_LONG = "exit_long_signal"
EXIT_SHORT = "exit_short_signal"
STRENGTH = "signal_strength"
BAR_INDEX = "bar_index"
LEDGER = (
    "entry_bar", "exit_bar", "side", "quantity", "entry_price",
    "exit_price", "commission", "pnl", "exit_reason",
)
RULE_CODE = {
    "fixed_sl": 0, "atr_sl": 1, "fixed_tp": 2, "atr_tp": 3, "trailing": 4,
    "chandelier": 5, "breakeven": 6, "psar": 7, "profit_target_ratchet": 8,
    "time_stop": 9, "donchian_stop": 10,
}


def load_case(family, case_id):
    for path in (FIXTURES / family).glob("*.json"):
        data = json.loads(path.read_text(encoding="utf-8"))
        for case in data["cases"]:
            if case["case_id"] == case_id:
                return case
    raise KeyError(case_id)


def col(encoded):
    return np.asarray(encoded["values"], dtype=encoded["dtype"])


def decode_inputs(case):
    out = {}
    for name, encoded in case["inputs"].items():
        if name in (EXIT_LONG, EXIT_SHORT, "tradable"):
            out[name] = col(encoded).astype(bool)
        elif name == ENTRY:
            out[name] = col(encoded).astype(np.int8)
        else:
            out[name] = col(encoded)
    return out


def indicators(inputs):
    return {
        k: v for k, v in inputs.items()
        if k.startswith("atr_") or k.startswith("donchian_")
    }


def run_fixture(case):
    inputs = decode_inputs(case)
    config = case["config"]
    costs = config.get("costs")
    costs_tuple = None if costs is None else (costs["per_contract"], costs["bps"])
    day = config.get("day_trade_us")
    day_tuple = None if day is None else tuple(day)
    return engine.run_candle(
        time_us=inputs["time_us"],
        open=inputs.get("open"),
        high=inputs.get("high"),
        low=inputs.get("low"),
        close=inputs.get("close"),
        entry=inputs[ENTRY],
        exit_long=inputs[EXIT_LONG],
        exit_short=inputs[EXIT_SHORT],
        strength=inputs[STRENGTH],
        bar_index=inputs.get(BAR_INDEX),
        volatility=inputs.get("volatility"),
        tradable=inputs.get("tradable"),
        columns=indicators(inputs),
        initial_capital=config["initial_capital"],
        point_value=config["point_value"],
        costs=costs_tuple,
        sizing=config["sizing"],
        sizing_point_value=config["sizing_point_value"],
        holding_period_bars=config.get("holding_period_bars"),
        exit_params=config.get("exit_params", {}),
        day_trade_us=day_tuple,
        force_close_at_end=config["force_close_at_end"],
    )


# required_columns / enabled_rules / max_position
assert engine.required_columns({
    "stop_loss_atr": 2,
    "atr_period": "21",
    "donchian_exit_period": 10.0,
}) == ["atr_21", "donchian_high_10", "donchian_low_10"]
assert engine.enabled_rules({"trailing_stop_pct": 0.03, "max_bars_in_trade": 5}) == [
    "trailing", "time_stop",
]
inv = {
    "type": "inverse_volatility",
    "target_volatility_pct": 10.0,
    "min_contracts": 0,
    "max_contracts": 5,
    "scale_by_signal_strength": False,
}
assert engine.max_position(inv, point_value=0.2, price=100.0, capital=100_000.0) == 5.0
inv.pop("max_contracts")
assert engine.max_position(inv, point_value=0.2, price=100.0, capital=100_000.0) is None

# k07 ledger parity
k07 = load_case("candle_engine", "k07_safety_margin_compounding")
k07_out = run_fixture(k07)
float_cols = {"quantity", "entry_price", "exit_price", "commission", "pnl"}
for name in LEDGER:
    got = np.asarray(k07_out[name])
    want = col(k07["expected"][name])
    if name in float_cols:
        assert np.allclose(got, want, equal_nan=True), name
    else:
        assert np.array_equal(got, want), name

# k02 exit_reason_text for rule exits
k02 = load_case("candle_engine", "k02_stop_target_rules")
k02_out = run_fixture(k02)
texts = k02_out["exit_reason_text"]
codes = np.asarray(k02_out["exit_reason"])
assert any(t == "fixed_sl" for t in texts if t is not None)
assert any(t == "fixed_tp" for t in texts if t is not None)
for code, text in zip(codes, texts):
    if code < 0:
        assert text is None
    elif 0 <= code <= 10:
        assert text is not None

# d03 DecisionStep bar-by-bar
d03 = load_case("decision_step", "d03_short_open_scripted_exit")
inputs = decode_inputs(d03)
config = d03["config"]
trade = config["open_trade"]
step = engine.DecisionStep(exit_params=config.get("exit_params", {}))
open_trade = (trade["id"], trade["side"], trade["entry_price"], trade["entry_bar"])
exit_offsets = [0]
exit_reason = []
entry = []
entry_strength = []
for bar in range(len(inputs["close"])):
    exits, queued = step.decide(
        bar=bar,
        trades=[open_trade],
        close=inputs["close"],
        high=inputs.get("high"),
        low=inputs.get("low"),
        entry=inputs[ENTRY],
        exit_long=inputs[EXIT_LONG],
        exit_short=inputs[EXIT_SHORT],
        strength=inputs[STRENGTH],
        bar_index=inputs.get(BAR_INDEX),
        columns=indicators(inputs),
        holding_period_bars=config.get("holding_period_bars"),
    )
    for _tid, reason in exits:
        exit_reason.append(RULE_CODE[reason] if reason is not None else 11)
    exit_offsets.append(len(exit_reason))
    if queued is None:
        entry.append(0)
        entry_strength.append(0.0)
    else:
        side, strength = queued
        entry.append(side)
        entry_strength.append(strength)
exp = d03["expected"]
assert exit_offsets == col(exp["exit_offsets"]).tolist()
assert exit_reason == col(exp["exit_reason"]).tolist()
assert entry == col(exp["entry"]).tolist()
assert entry_strength == col(exp["entry_strength"]).tolist()

# trailing state after three decide calls
d04 = load_case("decision_step", "d04_trailing_atr_stop_long")
inputs = decode_inputs(d04)
config = d04["config"]
trade = config["open_trade"]
step = engine.DecisionStep(exit_params=config["exit_params"])
open_trade = (trade["id"], trade["side"], trade["entry_price"], trade["entry_bar"])
extreme = trade["entry_price"]
for bar in range(3):
    step.decide(
        bar=bar,
        trades=[open_trade],
        close=inputs["close"],
        high=inputs["high"],
        low=inputs["low"],
        entry=inputs[ENTRY],
        exit_long=inputs[EXIT_LONG],
        exit_short=inputs[EXIT_SHORT],
        strength=inputs[STRENGTH],
        columns=indicators(inputs),
    )
    extreme = max(extreme, inputs["high"][bar])
state = step.state("golden-parity-trade")
assert state is not None and state["trailing"]["extreme"] == extreme
assert step.state("unknown") is None

# dtype / length errors
n = 400
base = dict(
    time_us=np.zeros(n, dtype=np.int64),
    entry=np.zeros(n, dtype=np.int8),
    exit_long=np.zeros(n, dtype=bool),
    exit_short=np.zeros(n, dtype=bool),
    strength=np.ones(n, dtype=np.float64),
    columns={},
    initial_capital=100_000.0,
    point_value=1.0,
    sizing={"type": "fixed_quantity", "quantity": 1.0, "scale_by_signal_strength": False},
    sizing_point_value=1.0,
    exit_params={},
    force_close_at_end=False,
)
try:
    engine.run_candle(**{**base, "close": np.ones(n, dtype=np.float16)})
except ValueError as exc:
    assert "close" in str(exc)
else:
    raise AssertionError("expected ValueError for float16 close")
try:
    engine.run_candle(**{**base, "close": np.ones(n, dtype=np.float64), "strength": np.ones(399)})
except ValueError as exc:
    assert "strength" in str(exc)
else:
    raise AssertionError("expected ValueError for strength length")
try:
    engine.run_candle(**{**base, "close": np.ones(n, dtype=np.float64), "entry": np.zeros(n, dtype=np.int64)})
except TypeError as exc:
    assert "entry" in str(exc)
else:
    raise AssertionError("expected TypeError for int64 entry")

# intrabar stop and target orders: the same ledgers as protective_orders_gate.rs
assert engine.PROTECTIVE_ORDERS is True
HOUR = 3_600_000_000
SECOND = 1_000_000


def bar_times(bar, count):
    return [(10 + bar) * HOUR + i * SECOND for i in range(count)]


def protective_run(open_, high, low, entry, stop, target, intrabar, entry_price=None):
    count = len(open_)
    return engine.run_candle(
        time_us=np.array([(10 + b) * HOUR for b in range(count)], dtype=np.int64),
        open=np.array(open_, dtype=np.float64),
        high=np.array(high, dtype=np.float64),
        low=np.array(low, dtype=np.float64),
        close=None,
        entry=np.array(entry, dtype=np.int8),
        exit_long=np.zeros(count, dtype=bool),
        exit_short=np.zeros(count, dtype=bool),
        strength=np.array([1.0 if e else 0.0 for e in entry], dtype=np.float64),
        bar_index=None,
        volatility=None,
        tradable=None,
        columns={},
        initial_capital=10_000.0,
        point_value=1.0,
        costs=None,
        sizing={"type": "fixed_quantity", "quantity": 1.0, "scale_by_signal_strength": False},
        sizing_point_value=1.0,
        holding_period_bars=None,
        exit_params={},
        day_trade_us=None,
        force_close_at_end=False,
        stop_price=None if stop is None else np.array(stop, dtype=np.float64),
        target_price=None if target is None else np.array(target, dtype=np.float64),
        entry_price=None if entry_price is None else np.array(entry_price, dtype=np.float64),
        intrabar=intrabar,
    )


def tape_source(bars, calls):
    # Serves each listed bar's prices and records every call; any other bar fails the run.

    def source(bar):
        calls.append(bar)
        if bar not in bars:
            raise AssertionError(f"unexpected call for bar {bar}")
        prices = bars[bar]
        return (
            np.array(bar_times(bar, len(prices)), dtype=np.int64),
            np.array(prices, dtype=np.float64),
        )

    return source


NAN = float("nan")
# Criterion 1: a long closes at its stop; the first price at or below 95 fills there.
calls = []
out = protective_run(
    [100, 100, 100], [100, 101, 101], [100, 100, 94], [1, 0, 0],
    [95, NAN, NAN], [110, NAN, NAN],
    tape_source({2: [101, 96, 95, 94]}, calls),
)
assert out["exit_bar"].tolist() == [2]
assert out["exit_price"].tolist() == [95.0]
assert out["exit_reason"].tolist() == [14]
assert out["exit_reason_text"] == ["STOP_LOSS"]
assert out["exit_time_us"].tolist() == [bar_times(2, 3)[2]]
assert calls == [2], calls

# Criterion 2: a bar holding both levels resolves by price order.
for prices, want_price, want_reason, want_text in [
    ([100, 111, 94], 110.0, 15, "TAKE_PROFIT"),
    ([100, 94, 111], 94.0, 14, "STOP_LOSS"),
]:
    out = protective_run(
        [100, 100, 100], [100, 100, 111], [100, 100, 94], [1, 0, 0],
        [95, NAN, NAN], [110, NAN, NAN],
        tape_source({2: prices}, []),
    )
    assert out["exit_price"].tolist() == [want_price], prices
    assert out["exit_reason"].tolist() == [want_reason], prices
    assert out["exit_reason_text"] == [want_text], prices

# Criterion 6: a long whose stop sits at its fill is not opened and is reported.
calls = []
out = protective_run(
    [100, 100], [100, 101], [100, 99], [1, 0],
    [100, NAN], [NAN, NAN],
    tape_source({}, calls),
)
assert len(out["entry_bar"]) == 0
assert out["rejected_bar"].tolist() == [1]
assert out["rejected_side"].tolist() == [1]
assert out["rejected_fill_price"].tolist() == [100.0]
assert out["rejected_stop_price"].tolist() == [100.0]
assert out["rejected_reason"].tolist() == [0]
assert np.isnan(out["rejected_target_price"][0])
assert calls == [], calls

# Criterion 11: an exception raised by the callable reaches the caller unchanged.
class Boom(RuntimeError):
    pass


def raising(bar):
    raise Boom("feed unavailable")


try:
    protective_run(
        [100, 100, 100], [100, 101, 101], [100, 100, 94], [1, 0, 0],
        [95, NAN, NAN], [110, NAN, NAN], raising,
    )
except Boom as exc:
    assert str(exc) == "feed unavailable"
else:
    raise AssertionError("expected the callable's Boom to propagate")

# Levels need the callable, and stop and target travel together.
try:
    protective_run(
        [100, 100, 100], [100, 101, 101], [100, 100, 94], [1, 0, 0],
        [95, NAN, NAN], [110, NAN, NAN], None,
    )
except ValueError as exc:
    assert "protective" in str(exc)
else:
    raise AssertionError("expected ValueError without an intrabar callable")
try:
    engine.run_candle(
        time_us=np.zeros(3, dtype=np.int64), open=np.ones(3), high=np.ones(3), low=np.ones(3),
        close=None, entry=np.zeros(3, dtype=np.int8), exit_long=np.zeros(3, dtype=bool),
        exit_short=np.zeros(3, dtype=bool), strength=np.zeros(3), columns={},
        initial_capital=1.0, point_value=1.0, costs=None,
        sizing={"type": "fixed_quantity", "quantity": 1.0, "scale_by_signal_strength": False},
        sizing_point_value=1.0, exit_params={}, force_close_at_end=False,
        stop_price=np.ones(3),
    )
except ValueError as exc:
    assert "stop_price" in str(exc) and "target_price" in str(exc)
else:
    raise AssertionError("expected ValueError for a lone stop_price")

# Runtime decisions must observe fills, preserve exceptions, and own input buffers.
callback_prices = np.array([100., 101., 102., 103.])
callback_kwargs = dict(
    time_us=np.arange(4, dtype=np.int64), open=callback_prices, high=callback_prices,
    low=callback_prices, close=callback_prices, entry=np.zeros(4, dtype=np.int8),
    exit_long=np.zeros(4, dtype=bool), exit_short=np.zeros(4, dtype=bool),
    strength=np.zeros(4), columns={}, initial_capital=1000., point_value=1., costs=None,
    sizing={"type": "fixed_quantity", "quantity": 2., "scale_by_signal_strength": False},
    sizing_point_value=1., exit_params={}, force_close_at_end=False,
)
seen = []
def callback(bar, positions):
    seen.append(positions)
    callback_prices[:] = -999.  # cannot affect simulation buffers
    return (1 if bar == 0 else 0, bar == 1, bar == 1, 1. if bar == 0 else 0.)
run = engine.run_candle(**callback_kwargs, strategy_callback=callback)
assert seen[0] == ()
assert seen[1] == ((0, 1, 1, 101., 2.),)
assert seen[2] == ()
assert run["entry_price"].tolist() == [101.]
assert run["exit_price"].tolist() == [102.]
marker = RuntimeError("callback marker")
def fail(*args):
    raise marker
try:
    engine.run_candle(**callback_kwargs, strategy_callback=fail)
except RuntimeError as exc:
    assert exc is marker
else:
    raise AssertionError("callback exception swallowed")

# Explicit exit prices close on the first qualifying trade after the closed-bar decision.
exit_calls = []
def explicit_exit(bar, positions):
    if bar == 0:
        return {"entry": 1, "strength": 1.}
    if bar == 2:
        return {"exit_long": True, "exit_price": 103.}
    return {}
explicit_exit_run = engine.run_candle(
    time_us=np.array([10 * HOUR, 11 * HOUR, 12 * HOUR, 13 * HOUR], dtype=np.int64),
    open=np.array([100., 100., 100., 120.]), high=np.array([100., 100., 106., 120.]),
    low=np.array([100., 100., 99., 120.]), close=None,
    entry=np.array([1, 0, 0, 0], dtype=np.int8), exit_long=np.zeros(4, dtype=bool),
    exit_short=np.zeros(4, dtype=bool), strength=np.array([1., 0., 0., 0.]), columns={},
    initial_capital=1000., point_value=1., costs=None,
    sizing={"type": "fixed_quantity", "quantity": 1., "scale_by_signal_strength": False},
    sizing_point_value=1., exit_params={}, force_close_at_end=False,
    strategy_callback=explicit_exit,
    intrabar=tape_source({2: [105., 103., 101.]}, exit_calls),
)
assert explicit_exit_run["exit_price"].tolist() == [103.]
assert explicit_exit_run["exit_time_us"].tolist() == [12 * HOUR + SECOND]
assert explicit_exit_run["exit_reason_text"] == ["SIGNAL"]
assert exit_calls == [2]

for decision in [(2, False, False, 0.), (1, False, False, float("nan")), (0, 1, False, 0.), None]:
    try:
        engine.run_candle(**callback_kwargs, strategy_callback=lambda *args: decision)
    except (TypeError, ValueError):
        pass
    else:
        raise AssertionError(f"accepted invalid callback decision: {decision}")
# Shared runtime-callback candle fixture for priced entries and protection.
def callback_run(strategy_callback=None, bar_two_low=99., **extra):
    return engine.run_candle(
        time_us=np.array([(10 + b) * HOUR for b in range(4)], dtype=np.int64),
        open=np.array([100., 100., 100., 120.]), high=np.array([100., 100., 106., 120.]),
        low=np.array([100., 100., bar_two_low, 120.]), close=None,
        entry=np.array([1, 0, 0, 0], dtype=np.int8), exit_long=np.zeros(4, dtype=bool),
        exit_short=np.zeros(4, dtype=bool), strength=np.array([1., 0., 0., 0.]), columns={},
        initial_capital=1000., point_value=1., costs=None,
        sizing={"type": "fixed_quantity", "quantity": 1., "scale_by_signal_strength": False},
        sizing_point_value=1., exit_params={}, force_close_at_end=False,
        strategy_callback=strategy_callback, **extra,
    )

# A price the tape never reaches consumes this decision without a next-open fallback.
unreached = callback_run(
    strategy_callback=lambda bar, positions: (
        {"entry": 1, "strength": 1.} if bar == 0 else
        {"exit_long": True, "exit_price": 102.} if bar == 2 else {}
    ),
    intrabar=tape_source({2: [105., 103., 102.5]}, []),
)
assert unreached["exit_bar"].tolist() == [-1]

for bad_price in [0., -1., float("inf"), 107.]:
    try:
        callback_run(
            strategy_callback=lambda bar, positions, value=bad_price:
                ({"entry": 1, "strength": 1.} if bar == 0 else
                 {"exit_long": True, "exit_price": value} if bar == 2 else {}),
            intrabar=tape_source({2: [105., 103., 101.]}, []),
        )
    except ValueError as exc:
        assert "exit_price" in str(exc)
    else:
        raise AssertionError(f"accepted invalid exit price: {bad_price}")
try:
    callback_run(strategy_callback=lambda bar, positions: (
        {"entry": 1, "strength": 1.} if bar == 0 else
        {"exit_long": True, "exit_price": 103.} if bar == 2 else {}
    ))
except ValueError as exc:
    assert "exit_price" in str(exc) and "intrabar" in str(exc)
else:
    raise AssertionError("explicit exit price accepted without intrabar data")
# A close requested once a priced entry fills exits at the next open, ahead of that bar's target.
def fill_run(entry_fill_callback):
    return engine.run_candle(
        time_us=np.array([(10 + b) * HOUR for b in range(3)], dtype=np.int64),
        open=np.array([100., 100., 100.]), high=np.array([100., 101., 107.]),
        low=np.array([100., 99., 100.]), close=None,
        entry=np.zeros(3, dtype=np.int8), exit_long=np.zeros(3, dtype=bool),
        exit_short=np.zeros(3, dtype=bool), strength=np.zeros(3), columns={},
        initial_capital=1000., point_value=1., costs=None,
        sizing={"type": "fixed_quantity", "quantity": 1., "scale_by_signal_strength": False},
        sizing_point_value=1., exit_params={}, force_close_at_end=False,
        strategy_callback=lambda bar, positions: (
            (1, False, False, 1., NAN, 106., 100.) if bar == 1 else (0, False, False, 0.)
        ),
        entry_fill_callback=entry_fill_callback,
        intrabar=tape_source({1: [100., 101., 99.], 2: [100., 103., 107.]}, []),
    )
fill_calls = []
closed_after_fill = fill_run(lambda bar, positions: fill_calls.append((bar, positions)) is None)
assert [(bar, len(positions)) for bar, positions in fill_calls] == [(1, 1)]
assert closed_after_fill["exit_bar"].tolist() == [2]
assert closed_after_fill["exit_price"].tolist() == [100.]
assert closed_after_fill["exit_reason_text"] == ["SIGNAL"]
assert fill_run(lambda bar, positions: False)["exit_reason_text"] == ["TAKE_PROFIT"]
try:
    fill_run(lambda bar, positions: 1)
except TypeError as exc:
    assert "entry_fill_callback" in str(exc)
else:
    raise AssertionError("accepted a non-bool entry_fill_callback result")
# Order lists: the first exit price the tape reaches closes the trade, and a split entry callback
# sees the positions left after it and fills the first entry candidate the tape touches.
def lists_run(strategy_callback, entry_callback=None):
    return engine.run_candle(
        time_us=np.array([(10 + b) * HOUR for b in range(3)], dtype=np.int64),
        open=np.array([100., 100., 102.]), high=np.array([100., 106., 102.]),
        low=np.array([100., 94., 102.]), close=None,
        entry=np.zeros(3, dtype=np.int8), exit_long=np.zeros(3, dtype=bool),
        exit_short=np.zeros(3, dtype=bool), strength=np.zeros(3), columns={},
        initial_capital=1000., point_value=1., costs=None,
        sizing={"type": "fixed_quantity", "quantity": 1., "scale_by_signal_strength": False},
        sizing_point_value=1., exit_params={}, force_close_at_end=False,
        strategy_callback=strategy_callback, entry_callback=entry_callback,
        intrabar=tape_source({1: [100., 94., 104., 106.]}, []),
    )
first_reached = lists_run(lambda bar, positions: (
    {"entry": 1, "strength": 1.} if bar == 0 else
    {"exit_long": True, "exit_prices": [105., 95.]} if bar == 1 else {}
))
# The long's stop at 95 is reached by the jump to 94, which is its fill.
assert first_reached["exit_price"].tolist() == [94.]
assert first_reached["exit_candidate"].tolist() == [1]
assert first_reached["entry_candidate"].tolist() == [-1]
entry_seen = []
def split_entry(bar, positions):
    entry_seen.append((bar, len(positions)))
    if bar == 0:
        return {"entry": 1, "strength": 1.}
    if bar == 1:
        return {"entry": 1, "strength": 1.,
                "entry_candidates": [(1, 105., NAN, NAN), (-1, 103., 110., 90.)]}
    return {}
split = lists_run(
    lambda bar, positions: {"exit_long": True, "exit_prices": [95.]} if bar == 1 else {},
    split_entry,
)
assert entry_seen[:2] == [(0, 0), (1, 0)]
assert split["side"].tolist() == [1, -1]
assert split["entry_price"].tolist() == [100., 103.]
assert split["entry_candidate"].tolist() == [-1, 1]
assert split["final_stop_price"].tolist()[1] == 110.
for strategy_callback, entry_callback, message in [
    (lambda bar, positions: {"exit_long": True, "exit_price": 95., "exit_prices": [95.]}, None,
     "exit_prices"),
    (lambda bar, positions: {"entry": 1, "entry_candidates": [(0, 100., NAN, NAN)]}, None,
     "entry_candidates"),
    (lambda bar, positions: {"entry": 1, "entry_price": 100., "entry_candidates": []}, None,
     "entry_candidates"),
    (lambda bar, positions: {"entry": 1, "strength": 1.}, lambda bar, positions: {}, "entry"),
    (lambda bar, positions: {}, lambda bar, positions: {"exit_long": True}, "exit_long"),
]:
    try:
        lists_run(strategy_callback, entry_callback)
    except ValueError as exc:
        assert message in str(exc), exc
    else:
        raise AssertionError(f"accepted an invalid order list for {message}")
# Runtime levels: the six-value decision carries the stop, and a stop fills at its traded price.
calls = []
def levelled(bar, positions):
    if bar == 0:
        return (1, False, False, 1., 95., 110.)
    return (0, False, False, 0., NAN, NAN)
run = callback_run(strategy_callback=levelled, bar_two_low=94., intrabar=tape_source({2: [101., 94.]}, calls))
assert run["exit_reason_text"] == ["STOP_LOSS"], run["exit_reason_text"]
assert run["exit_price"].tolist() == [94.]
try:
    callback_run(strategy_callback=levelled)
except ValueError as exc:
    assert "stop_price" in str(exc)
else:
    raise AssertionError("runtime levels accepted without an intrabar source")
try:
    callback_run(strategy_callback=lambda bar, positions: (0, False, False, 0., -1., NAN),
                 intrabar=tape_source({}, []))
except ValueError as exc:
    assert "stop_price" in str(exc)
else:
    raise AssertionError("non-positive runtime level accepted")
try:
    callback_run(strategy_callback=lambda bar, positions: (0, False, False, 0., 1., 2., 3., 4., 5.),
                 intrabar=tape_source({}, []))
except TypeError as exc:
    assert "four, six, seven or eight" in str(exc)
else:
    raise AssertionError("nine-value decision accepted")

# Priced entries: the ledgers of priced_entries_gate.rs criteria 1 and 4, and the range error.
out = protective_run(
    [100, 100], [101, 102], [99, 99.5], [0, 1], None, None, None, entry_price=[NAN, 101],
)
assert out["entry_bar"].tolist() == [1], out["entry_bar"]
assert out["entry_price"].tolist() == [101.0]
assert out["exit_bar"].tolist() == [-1]
assert out["entry"].tolist() == [0, 1]
assert len(out["rejected_bar"]) == 0

calls = []
out = protective_run(
    [99], [101.5], [99], [1], [NAN], [101], tape_source({0: [99, 100, 99.5, 101.5]}, calls),
    entry_price=[100],
)
assert out["entry_price"].tolist() == [100.0]
assert out["exit_bar"].tolist() == [0]
assert out["exit_price"].tolist() == [101.0]
assert out["exit_reason"].tolist() == [15]
assert out["exit_time_us"].tolist() == [bar_times(0, 4)[3]]
assert calls == [0], calls

try:
    protective_run([100], [101], [99], [1], None, None, None, entry_price=[102])
except ValueError as exc:
    assert "entry_price[0]" in str(exc) and "outside the bar range" in str(exc), str(exc)
else:
    raise AssertionError("priced entry above the bar range accepted")

# A runtime decision carries its own price, which is range-checked like a static one.
def priced_decision(bar, positions):
    return (1, False, False, 1., NAN, NAN, 100.) if bar == 1 else (0, False, False, 0.)
run = callback_run(strategy_callback=priced_decision)
assert run["entry_bar"].tolist() == [1], run["entry_bar"]
assert run["entry_price"].tolist() == [100.0]
try:
    callback_run(strategy_callback=lambda bar, positions: (1, False, False, 1., NAN, NAN, -1.)
                 if bar == 1 else (0, False, False, 0.))
except ValueError as exc:
    assert "entry_price" in str(exc)
else:
    raise AssertionError("non-positive runtime entry price accepted")

# Removed per-tick callbacks are no longer accepted by the Python API.
try:
    callback_run(exit_screen_callback=lambda *args: True)
except TypeError:
    pass
else:
    raise AssertionError("removed exit callback parameter was accepted")

# Native sources are exposed separately from Python callables and preserve typed data failures.
assert engine.SESSION_TICK_SOURCE is True
native_source = engine.SessionTickSource(
    [None], np.array([0], dtype=np.int64), np.array([10], dtype=np.int64),
    np.array([11], dtype=np.int64), np.array([100.]), np.array([101.]),
    np.array([99.]), np.array([100.]),
)
try:
    native_source.prices(0)
except engine.IntrabarDataError as exc:
    assert exc.bar == 0 and exc.reason == "missing_session" and exc.session == 0
    assert exc.observed is None and exc.expected is None
else:
    raise AssertionError("missing session did not raise IntrabarDataError")

# `run_candle` dispatches the source natively while checking a later bar's target.
# The source is deliberately not callable: falling back through Python would fail here.
native_run_source = engine.SessionTickSource(
    [None], np.array([0, 0], dtype=np.int64), np.array([10, 11], dtype=np.int64),
    np.array([11, 12], dtype=np.int64), np.array([100., 100.]), np.array([100., 102.]),
    np.array([100., 99.]), np.array([100., 101.]),
)
try:
    protective_run([100., 100.], [100., 102.], [100., 99.], [1, 0], [NAN, NAN],
                   [101., NAN], native_run_source)
except engine.IntrabarDataError as exc:
    assert exc.bar == 1 and exc.reason == "missing_session", (exc.bar, exc.reason)
else:
    raise AssertionError("run_candle did not call the native source on the screened bar")

# File decoding failures retain their path and are not mislabeled as missing sessions.
bad_file_source = engine.SessionTickSource(
    ["/tmp/q119-session-that-does-not-exist.parquet"], np.array([0], dtype=np.int64),
    np.array([10], dtype=np.int64), np.array([11], dtype=np.int64), np.array([100.]),
    np.array([101.]), np.array([99.]), np.array([100.]),
)
try:
    bad_file_source.prices(0)
except engine.IntrabarDataError:
    raise AssertionError("Parquet read failure was mislabeled as IntrabarDataError")
except ValueError as exc:
    assert "/tmp/q119-session-that-does-not-exist.parquet" in str(exc), str(exc)
else:
    raise AssertionError("missing Parquet file did not fail")

# Native reads filter and stably sort a tick-store session, then drive the same
# protective-order results as the existing Python callable source.
with tempfile.TemporaryDirectory() as tmpdir:
    session_path = Path(tmpdir) / "session.parquet"
    shutil.copyfile(ROOT / "tests" / "fixtures" / "session_ticks.parquet", session_path)
    parquet_source = engine.SessionTickSource(
        [str(session_path)], np.array([0, 0], dtype=np.int64),
        np.array([10, 13], dtype=np.int64), np.array([13, 15], dtype=np.int64),
        np.array([100., 99.]), np.array([101., 102.]), np.array([100., 99.]),
        np.array([101., 102.]),
    )
    times0, prices0 = parquet_source.prices(0)
    times1, prices1 = parquet_source.prices(1)
    raw_time_msc = np.array([12, 10, 11, 11, 13, 14, 15], dtype=np.int64)
    raw_last = np.array([101., 100., 100.5, 100.75, 99., 102., NAN], dtype=np.float64)
    valid = np.isfinite(raw_last) & (raw_last > 0)
    expected_time_msc, expected_last = raw_time_msc[valid], raw_last[valid]
    if expected_time_msc.size > 1 and np.any(expected_time_msc[1:] < expected_time_msc[:-1]):
        order = np.argsort(expected_time_msc, kind="stable")
        expected_time_msc, expected_last = expected_time_msc[order], expected_last[order]
    np.testing.assert_array_equal(times0, expected_time_msc[expected_time_msc < 13] * 1000)
    np.testing.assert_array_equal(prices0, expected_last[expected_time_msc < 13])
    np.testing.assert_array_equal(times1, expected_time_msc[(expected_time_msc >= 13) & (expected_time_msc < 15)] * 1000)
    np.testing.assert_array_equal(prices1, expected_last[(expected_time_msc >= 13) & (expected_time_msc < 15)])
    native_run = protective_run(
        [100., 99.], [101., 102.], [100., 99.], [1, 0], [NAN, NAN],
        [101.5, NAN], parquet_source,
    )
    callable_calls = []
    def python_source(bar):
        callable_calls.append(bar)
        return parquet_source.prices(bar)
    callable_run = protective_run(
        [100., 99.], [101., 102.], [100., 99.], [1, 0], [NAN, NAN],
        [101.5, NAN], python_source,
    )
    assert callable_calls == [1], callable_calls
    assert native_run.keys() == callable_run.keys()
    for name in native_run:
        np.testing.assert_equal(native_run[name], callable_run[name], err_msg=name)
    assert native_run["exit_price"].tolist() == [101.5]

    # Q-105 priced entries use the deciding bar's native tape.
    native_priced = protective_run(
        [100.], [101.], [100.], [1], [99.5], [100.6], parquet_source,
        entry_price=[100.25],
    )
    priced_calls = []
    def python_priced_source(bar):
        priced_calls.append(bar)
        return parquet_source.prices(bar)
    callable_priced = protective_run(
        [100.], [101.], [100.], [1], [99.5], [100.6], python_priced_source,
        entry_price=[100.25],
    )
    assert priced_calls == [0], priced_calls
    for name in native_priced:
        np.testing.assert_equal(native_priced[name], callable_priced[name], err_msg=name)
    # The buy at 100.25 is a stop above the 100 open; the tape jumps to 100.5, its fill.
    assert native_priced["entry_price"].tolist() == [100.5]
    assert native_priced["exit_price"].tolist() == [100.6]

    mismatch_source = engine.SessionTickSource(
        [str(session_path)], np.array([0], dtype=np.int64), np.array([10], dtype=np.int64),
        np.array([13], dtype=np.int64), np.array([100.]), np.array([101.1]),
        np.array([100.]), np.array([101.]),
    )
    try:
        mismatch_source.prices(0)
    except engine.IntrabarDataError as exc:
        assert exc.bar == 0 and exc.reason == "mismatch" and exc.session is None
        assert tuple(exc.observed) == (100., 101., 100., 101.)
        assert tuple(exc.expected) == (100., 101.1, 100., 101.)
    else:
        raise AssertionError("OHLC mismatch did not raise IntrabarDataError")

    empty_source = engine.SessionTickSource(
        [str(session_path)], np.array([0], dtype=np.int64), np.array([15], dtype=np.int64),
        np.array([16], dtype=np.int64), np.array([100.]), np.array([100.]),
        np.array([100.]), np.array([100.]),
    )
    try:
        empty_source.prices(0)
    except engine.IntrabarDataError as exc:
        assert exc.bar == 0 and exc.reason == "empty" and exc.session is None
        assert exc.observed is None and exc.expected is None
    else:
        raise AssertionError("empty interval did not raise IntrabarDataError")

# Resting orders and amendments through the dict form: the ledgers match the Rust gate.
assert engine.RESTING_ORDERS is True
ORDER_TIMES = [(10 + b) * HOUR for b in range(4)]


def order_run(decisions, bars, intrabar=None, seen=None, **extra):
    def decide(bar, positions):
        if seen is not None:
            seen.append((bar, positions))
        return decisions.get(bar, {})

    count = len(bars)
    return engine.run_candle(
        time_us=np.array(ORDER_TIMES[:count], dtype=np.int64),
        open=np.array([b[0] for b in bars], dtype=np.float64),
        high=np.array([b[1] for b in bars], dtype=np.float64),
        low=np.array([b[2] for b in bars], dtype=np.float64),
        close=None,
        entry=np.zeros(count, dtype=np.int8), exit_long=np.zeros(count, dtype=bool),
        exit_short=np.zeros(count, dtype=bool), strength=np.zeros(count), columns={},
        initial_capital=1000., point_value=1., costs=None,
        sizing={"type": "fixed_quantity", "quantity": 1., "scale_by_signal_strength": False},
        sizing_point_value=1., exit_params={}, force_close_at_end=False,
        strategy_callback=decide, intrabar=intrabar, **extra,
    )


# A buy stop at 101 fills at the open of 102 when the bar gaps through it.
run = order_run({0: {"entry": 1, "entry_kind": "stop", "entry_price": 101.}},
                [(100., 100., 100.), (102., 103., 102.)],
                intrabar=tape_source({1: [102., 103.]}, []))
assert run["entry_price"].tolist() == [102.], run["entry_price"]
assert run["entry_kind"].tolist() == [2]
assert run["entry_time_us"].tolist() == [11 * HOUR]

# A stop-order trade opened at 101 stops out at 100 on the same bar.
calls = []
run = order_run({0: {"entry": 1, "entry_kind": "stop", "entry_price": 101., "stop_price": 100.}},
                [(100., 100., 100.), (100.5, 101., 100.)],
                intrabar=tape_source({1: [100.5, 101., 100.5, 100.]}, calls))
assert run["entry_price"].tolist() == [101.], run["entry_price"]
assert run["entry_time_us"].tolist() == [11 * HOUR + SECOND]
assert run["exit_bar"].tolist() == [1]
assert run["exit_price"].tolist() == [100.]
assert run["exit_reason_text"] == ["STOP_LOSS"], run["exit_reason_text"]
assert run["exit_time_us"].tolist() == [11 * HOUR + 3 * SECOND]

# A stop amended to 98 on bar 2 is not triggered by bar 2 and closes the trade on bar 3 at 98.
calls = []
run = order_run(
    {0: {"entry": 1, "stop_price": 95.}, 2: {"amend_stop": 98.}},
    [(100., 100., 100.), (100., 100., 100.), (100., 100.5, 97.5), (98.5, 99., 97.5)],
    intrabar=tape_source({3: [98.5, 98.]}, calls),
)
assert calls == [3], calls
assert run["exit_bar"].tolist() == [3]
assert run["exit_price"].tolist() == [98.], run["exit_price"]
assert run["exit_reason_text"] == ["STOP_LOSS"]
assert run["final_stop_price"].tolist() == [98.], run["final_stop_price"]

# Snapshots carry the levels in force when asked. NaN amendments keep a level.
seen = []
order_run(
    {0: {"entry": 1, "stop_price": 95., "target_price": 110.},
     1: {"amend_stop": NAN, "amend_target": 105.},
     2: {"amend_stop": 97.}},
    [(100., 100., 100.)] * 4,
    intrabar=tape_source({}, []), seen=seen, snapshot_levels=True,
)
snapshots = dict(seen)
assert snapshots[1] == ((0, 1, 1, 100., 1., 95., 110.),), snapshots[1]
assert snapshots[2] == ((0, 1, 1, 100., 1., 95., 105.),), snapshots[2]
assert snapshots[3] == ((0, 1, 1, 100., 1., 97., 105.),), snapshots[3]
seen = []
order_run({0: {"entry": 1}}, [(100., 100., 100.)] * 3, seen=seen)
assert dict(seen)[1] == ((0, 1, 1, 100., 1.),), seen

# Invalid dict decisions raise ValueError naming the bar and the key.
cases = [
    ({"entry": 1, "bogus": 1}, "bogus"),
    ({"entry": 1, "entry_kind": "stop"}, "entry_price"),
    ({"entry": 1, "entry_kind": "oco", "entry_price": 101.}, "entry_kind"),
    ({"entry": 1, "expire_bars": 2}, "expire_bars"),
    ({"entry": 1, "entry_kind": "limit", "entry_price": 99., "expire_bars": 0}, "expire_bars"),
    ({"amend_stop": -1.}, "amend_stop"),
    ({"entry": 1, "entry_kind": "stop", "entry_price": 101.}, "resting_order"),
]
for decision, needle in cases:
    intrabar = None if needle == "resting_order" else tape_source({}, [])
    try:
        order_run({0: decision}, [(100., 100., 100.)] * 2, intrabar=intrabar)
    except ValueError as exc:
        assert needle in str(exc), str(exc)
    else:
        raise AssertionError(f"accepted invalid decision: {decision}")

print("engine projection checks passed")
"""


if __name__ == "__main__":
    main()
