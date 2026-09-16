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

print("engine projection checks passed")
"""


if __name__ == "__main__":
    main()
