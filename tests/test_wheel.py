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
    "log_return", "realized_vol_per_bar", "ewma_vol_per_bar", "rogers_satchell",
    "true_range", "atr_fraction", "price_zscore_log", "ewma_trend_spread",
    "vol_normalize", "volatility_log_ratio", "quote_spread_bars",
    "volume_at_price", "volume_at_price_levels",
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

prev = 99.0
o = np.array([prev, 100.0, 103.0, 104.0], dtype=np.float64)
h = np.array([prev, 102.0, 106.0, 105.0], dtype=np.float64)
l = np.array([prev, 98.0, 102.0, 101.0], dtype=np.float64)
c = np.array([prev, 101.0, 105.0, 102.0], dtype=np.float64)
rs = qi.rogers_satchell(o[1:], h[1:], l[1:], c[1:], 3)
assert abs(rs[2] - 0.02533109341314) < 1e-12
yz = qi.yang_zhang(o, h, l, c, 3, 1)
assert abs(yz[3] - 0.02896649184455) < 1e-12
try:
    qi.log_return(c, 0)
except ValueError as e:
    assert "horizon" in str(e)
else:
    raise AssertionError("log_return invalid horizon must raise ValueError")

time = np.array([0], dtype=np.int64)
bid = np.array([99.9], dtype=np.float64)
ask = np.array([100.1], dtype=np.float64)
flags = np.array([6], dtype=np.int64)
session = np.array([1], dtype=np.int64)
bar_open = np.array([0], dtype=np.int64)
bar_close = np.array([60_000], dtype=np.int64)
spread = qi.quote_spread_bars(
    time, bid, ask, flags, session, bar_open, bar_close, 60_000
)
assert isinstance(spread, tuple) and len(spread) == 4
assert abs(spread[0][0] - 0.002) < 1e-12

# Ten seconds at 10 bps, then fifty at 20 bps, in one 60-second bar.
last, mean, peak, coverage = qi.quote_spread_bars(
    np.array([0, 10_000], dtype=np.int64),
    np.array([99.95, 99.9], dtype=np.float64),
    np.array([100.05, 100.1], dtype=np.float64),
    np.array([6, 6], dtype=np.int64),
    np.array([1, 1], dtype=np.int64),
    bar_open, bar_close, 120_000,
)
assert abs(mean[0] * 1e4 - (10 * 10 + 50 * 20) / 60) < 1e-9
assert abs(peak[0] * 1e4 - 20) < 1e-9 and coverage[0] == 1.0

# A quote before the bar counts only from the bar's open.
carry = qi.quote_spread_bars(
    np.array([-30_000], dtype=np.int64), bid, ask, flags, session, bar_open, bar_close, 120_000
)
assert carry[3][0] == 1.0

buy = 1 << 5
sell = 1 << 6
vap_time = np.array([0, 1, 2, 3], dtype=np.int64)
vap_price = np.array([100.0, 100.5, 100.0, 101.0], dtype=np.float64)
vap_vol = np.array([10.0, 5.0, 20.0, 3.0], dtype=np.float64)
vap_flags = np.array([buy, buy, sell, buy | sell], dtype=np.int64)
rows, segments = qi.volume_at_price(
    vap_time, vap_price, vap_vol, vap_flags,
    np.array([0], dtype=np.int64), np.array([10_000], dtype=np.int64),
    0.5, 1, 0.7,
)
assert isinstance(rows, dict) and isinstance(segments, dict)
assert abs(rows["total"][0] - 30.0) < 1e-12 and abs(segments["poc"][0] - 100.0) < 1e-12
vap_sess = np.array([1, 1, 1, 1], dtype=np.int64)
levels = qi.volume_at_price_levels(
    vap_time, vap_price, vap_vol, vap_flags, vap_sess,
    np.array([50], dtype=np.int64), np.array([1], dtype=np.int64),
    0.5, 1, 0.7,
)
assert isinstance(levels, dict)
assert abs(levels["total"][0] - segments["total"][0]) < 1e-12
assert abs(levels["poc"][0] - segments["poc"][0]) < 1e-12
try:
    qi.volume_at_price(
        vap_time, vap_price, vap_vol, vap_flags,
        np.array([0], dtype=np.int64), np.array([10_000], dtype=np.int64),
        0.5, 0, 0.7,
    )
except ValueError as e:
    assert "row_ticks" in str(e)
else:
    raise AssertionError("invalid row_ticks must raise ValueError")

# Preserve tape-order totals through the Python projection and keep the full
# traded range at a 100% value-area share even when the small row rounds away.
_, ordered_segments = qi.volume_at_price(
    np.array([0, 1, 2], dtype=np.int64), np.array([100., 101., 101.]),
    np.array([1e16, 1., 1.]), np.full(3, buy, dtype=np.int64),
    np.array([0], dtype=np.int64), np.array([3], dtype=np.int64), .5, 1, 1.,
)
assert ordered_segments["total"][0] == 1e16
assert ordered_segments["vah"][0] == 101.
try:
    qi.volume_at_price(
        np.array([0], dtype=np.int64), np.array([1e308]), np.array([1.]),
        np.array([buy], dtype=np.int64), np.array([0], dtype=np.int64),
        np.array([1], dtype=np.int64), .5, 1, .7,
    )
except ValueError as e:
    assert "price" in str(e)
else:
    raise AssertionError("unrepresentable tick index must raise ValueError")

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
