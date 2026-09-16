"""Wheel-level tests for q_core.engine tick projections."""

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
            ["uv", "pip", "install", "--python", str(python_bin), str(wheel_path), "numpy"],
            stdout=subprocess.DEVNULL,
        )
        code = CASES.replace("__ROOT__", repr(str(root)))
        res = subprocess.run(
            [str(python_bin), "-c", code],
            capture_output=True,
            text=True,
        )
        if res.returncode != 0:
            sys.stderr.write(res.stdout)
            sys.stderr.write(res.stderr)
            sys.exit(res.returncode)
        print(res.stdout.strip())
        print("Tick engine wheel tests passed successfully.")


CASES = r"""
import json
from pathlib import Path

import numpy as np
import q_core.engine as eng

ROOT = Path(__ROOT__)


def load_fixture(rel: str):
    with open(ROOT / "fixtures" / "reference" / rel, encoding="utf-8") as f:
        return json.load(f)


def f64(col):
    return np.array(
        [np.nan if v is None else float(v) for v in col["values"]],
        dtype=np.float64,
    )


def i64(col):
    return np.array(col["values"], dtype=np.int64)


def i8(col):
    return np.array(col["values"], dtype=np.int8)


def assert_col_eq(got, expected_col, name):
    if expected_col["dtype"] == "float64":
        exp = f64(expected_col)
        assert np.array_equal(got, exp, equal_nan=True), f"{name}: {got} != {exp}"
    else:
        exp = i64(expected_col)
        assert np.array_equal(got.astype(np.int64), exp), f"{name}: {got} != {exp}"


# --- tick_simulate on t09 ---
t09 = load_fixture("tick_kernel/t09_safety_margin_min_max.json")["cases"][0]
inp = t09["inputs"]
par = t09["params"]
out = eng.tick_simulate(
    bid=f64(inp["bid"]),
    ask=f64(inp["ask"]),
    direction=i8(inp["direction"]),
    sl_points=f64(inp["sl_points"]),
    tp_points=f64(inp["tp_points"]),
    initial_capital=float(par["initial_capital"]),
    point_value=float(par["point_value"]),
    sizing=dict(par["sizing"]),
)
exp = t09["expected"]
assert out["final_capital"] == exp["final_capital"]
for key in ("entry_idx", "exit_idx", "entry_price", "exit_price", "direction", "quantity", "exit_reason"):
    assert_col_eq(out[key], exp[key], key)

# --- tick_day_bounds equals t12 ---
t12 = load_fixture("tick_kernel/t12_day_bounds.json")
for case in t12["cases"]:
    starts, ends = eng.tick_day_bounds(i64(case["inputs"]["time_msc"]))
    assert_col_eq(starts, case["expected"]["starts"], f"{case['case_id']}.starts")
    assert_col_eq(ends, case["expected"]["ends"], f"{case['case_id']}.ends")

# --- tick_bars equals b01 ---
b01 = load_fixture("tick_bars/b01_last_price_m1.json")["cases"][0]
binp = b01["inputs"]
bars = eng.tick_bars(
    time_msc=i64(binp["time_msc"]),
    bid=f64(binp["bid"]),
    ask=f64(binp["ask"]),
    last=f64(binp["last"]),
    volume=f64(binp["volume"]),
    bar_ms=int(binp["bar_ms"]),
)
bexp = b01["expected"]
for key in ("open_msc", "open", "high", "low", "close", "volume", "tick_start", "tick_end"):
    assert_col_eq(bars[key], bexp[key], key)

# --- float32 bid → ValueError naming bid ---
n = 6
try:
    eng.tick_simulate(
        bid=np.zeros(n, dtype=np.float32),
        ask=np.zeros(n, dtype=np.float64),
        direction=np.zeros(n, dtype=np.int8),
        sl_points=np.full(n, np.nan),
        tp_points=np.full(n, np.nan),
        initial_capital=10_000.0,
        point_value=1.0,
        sizing={"type": "fixed_quantity", "quantity": 1.0},
    )
    raise SystemExit("expected ValueError for float32 bid")
except ValueError as e:
    assert "bid" in str(e), e

# --- ask len 10 vs bid len 11 → ValueError naming ask ---
try:
    eng.tick_simulate(
        bid=np.zeros(11, dtype=np.float64),
        ask=np.zeros(10, dtype=np.float64),
        direction=np.zeros(11, dtype=np.int8),
        sl_points=np.full(11, np.nan),
        tp_points=np.full(11, np.nan),
        initial_capital=10_000.0,
        point_value=1.0,
        sizing={"type": "fixed_quantity", "quantity": 1.0},
    )
    raise SystemExit("expected ValueError for ask length")
except ValueError as e:
    assert "ask" in str(e), e

# --- inverse_volatility sizing → ValueError naming inverse_volatility ---
try:
    eng.tick_simulate(
        bid=np.zeros(n, dtype=np.float64),
        ask=np.zeros(n, dtype=np.float64),
        direction=np.zeros(n, dtype=np.int8),
        sl_points=np.full(n, np.nan),
        tp_points=np.full(n, np.nan),
        initial_capital=10_000.0,
        point_value=1.0,
        sizing={
            "type": "inverse_volatility",
            "target_risk_fraction": 0.01,
            "volatility": 0.02,
            "min_contracts": 1,
            "max_contracts": 10,
        },
    )
    raise SystemExit("expected ValueError for inverse_volatility")
except ValueError as e:
    assert "inverse_volatility" in str(e), e

# --- direction int64 → TypeError ---
try:
    eng.tick_simulate(
        bid=np.zeros(n, dtype=np.float64),
        ask=np.zeros(n, dtype=np.float64),
        direction=np.zeros(n, dtype=np.int64),
        sl_points=np.full(n, np.nan),
        tp_points=np.full(n, np.nan),
        initial_capital=10_000.0,
        point_value=1.0,
        sizing={"type": "fixed_quantity", "quantity": 1.0},
    )
    raise SystemExit("expected TypeError for int64 direction")
except TypeError as e:
    assert "direction" in str(e), e

print("all tick engine cases ok")
"""


if __name__ == "__main__":
    main()
