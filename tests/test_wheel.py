import subprocess
import sys
import tempfile
from pathlib import Path

try:
    import tomllib
except ImportError:
    import tomli as tomllib


def main():
    root = Path(__file__).parents[1]
    cargo_toml = root / "Cargo.toml"
    contracts_rev_file = root / "CONTRACTS_REV"

    with open(cargo_toml, "rb") as f:
        data = tomllib.load(f)
    expected_version = data["workspace"]["package"]["version"]

    expected_rev = contracts_rev_file.read_text(encoding="utf-8").strip()

    dist = root / "dist"
    wheels = list(dist.glob("*.whl"))
    if not wheels:
        raise RuntimeError(f"No wheels found in {dist}. Run `make wheel` first.")
    wheel_path = sorted(wheels)[-1]
    print(f"wheel file: {wheel_path.name}")

    with tempfile.TemporaryDirectory() as tmpdir:
        venv_dir = Path(tmpdir) / "venv"
        subprocess.check_call(["uv", "venv", str(venv_dir)], stdout=subprocess.DEVNULL)
        python_bin = venv_dir / "bin" / "python"
        subprocess.check_call(
            ["uv", "pip", "install", "--python", str(python_bin), str(wheel_path)],
            stdout=subprocess.DEVNULL,
        )

        code = f"""
import numpy as np
import q_core
import q_core.indicators as qi

v = q_core.version()
c = q_core.contracts_rev()
print(f"wheel version: {{v}}")
print(f"wheel contracts_rev: {{c}}")
assert v == {expected_version!r}, f"Version mismatch: {{v}} != {expected_version!r}"
assert c == {expected_rev!r}, f"Contracts rev mismatch: {{c}} != {expected_rev!r}"

names = [
    "realized_vol", "yang_zhang", "rsi", "bollinger_bands", "macd",
    "donchian_channels", "atr", "sma", "ema", "smma", "wma", "hma",
    "rolling_zscore", "rolling_rank", "pct_change", "clip",
]
for name in names:
    assert hasattr(qi, name), f"missing q_core.indicators.{{name}}"

pct = qi.pct_change(np.array([0.0, 1.0, 0.0, 0.0], dtype=np.float64), 1)
assert np.isnan(pct[0]) and np.isinf(pct[1]) and pct[1] > 0
assert pct[2] == -1.0 and np.isnan(pct[3])

sma = qi.sma(np.array([1.0, 2.0, 3.0], dtype=np.float64), 2)
assert np.isnan(sma[0]) and sma[1] == 1.5 and sma[2] == 2.5

rsi = qi.rsi(np.arange(1.0, 20.0, dtype=np.float64), 14)
assert rsi[-1] == 100.0

x = np.arange(1.0, 30.0, dtype=np.float64)
x_before = x.copy()
out_sma = qi.sma(x, 2)
bb = qi.bollinger_bands(x, 20, 2.0)
assert isinstance(bb, tuple) and len(bb) == 3
dc = qi.donchian_channels(x + 1.0, x - 1.0, 10)
assert isinstance(dc, tuple) and len(dc) == 2
assert np.array_equal(x, x_before, equal_nan=True)
assert not np.shares_memory(out_sma, x)
assert not np.shares_memory(bb[0], x)

try:
    qi.rsi(x, 0)
except ValueError as e:
    assert "period" in str(e)
else:
    raise AssertionError("rsi(x, 0) must raise ValueError mentioning period")

try:
    qi.sma(np.array([1, 2, 3], dtype=np.int64), 2)
except TypeError:
    pass
else:
    raise AssertionError("int64 input must raise TypeError")

try:
    qi.sma(x[::2], 2)
except ValueError:
    pass
else:
    raise AssertionError("non-contiguous input must raise ValueError")

print("indicator projection checks passed")
"""
        res = subprocess.run([str(python_bin), "-c", code], capture_output=True, text=True)
        if res.returncode != 0:
            sys.stderr.write(res.stderr)
            sys.stdout.write(res.stdout)
            sys.exit(res.returncode)
        print(res.stdout.strip())
        print("Wheel integration test passed successfully.")


if __name__ == "__main__":
    main()
