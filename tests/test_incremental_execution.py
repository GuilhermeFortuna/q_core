"""Wheel-level tests for q_core.execution incremental execution primitives.

The vectors mirror crates/q-engine/tests/incremental_execution.rs so Rust and Python agree.
"""

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
    wheel_path = max(wheels)

    with tempfile.TemporaryDirectory() as tmpdir:
        venv_dir = Path(tmpdir) / "venv"
        subprocess.check_call(["uv", "venv", str(venv_dir)], stdout=subprocess.DEVNULL)
        python_bin = venv_dir / "bin" / "python"
        subprocess.check_call(
            ["uv", "pip", "install", "--python", str(python_bin), str(wheel_path)],
            stdout=subprocess.DEVNULL,
        )
        res = subprocess.run(
            [str(python_bin), "-c", CASES],
            capture_output=True,
            text=True,
            check=False,
        )
        if res.returncode != 0:
            sys.stderr.write(res.stdout)
            sys.stderr.write(res.stderr)
            sys.exit(res.returncode)
        print(res.stdout.strip())
        print("Incremental execution wheel tests passed successfully.")


CASES = r"""
import q_core
import q_core.execution as ex


def raises(exc, fn, *args, **kwargs):
    try:
        fn(*args, **kwargs)
    except exc:
        return
    except Exception as e:  # pragma: no cover - diagnostic
        raise SystemExit(f"expected {exc.__name__}, got {type(e).__name__}: {e}")
    raise SystemExit(f"expected {exc.__name__}, nothing raised")


def quote(side, kind, level, bid, ask, slip=0):
    return ex.match_quote(
        side=side, kind=kind, level_ticks=level, bid_ticks=bid, ask_ticks=ask, slippage_ticks=slip
    )


def trade(side, kind, level, trade_ticks):
    return ex.match_trade(side=side, kind=kind, level_ticks=level, trade_ticks=trade_ticks)


def fee(**overrides):
    inputs = dict(
        quantity_steps=3,
        volume_step="0.01",
        price_ticks=1000,
        tick_size="0.5",
        point_value="2",
        cost_per_contract="1.25",
        cost_bps="1.5",
        currency_decimals=2,
    )
    inputs.update(overrides)
    return ex.fill_cost(**inputs)


# Capability.
assert ex.INCREMENTAL_EXECUTION == 1
assert hasattr(q_core, "execution")

# Quote/trade distinction: ask = 100 and buy limit = 100 fill under quote policy only.
assert quote("buy", "limit", 100, 99, 100) == 100
assert trade("buy", "limit", 100, 100) is None
assert trade("buy", "limit", 100, 99) == 100

# Touch: stops trigger at the level, on both sides.
assert quote("buy", "stop", 100, 99, 100) == 100
assert quote("buy", "stop", 100, 98, 99) is None
assert quote("sell", "stop", 100, 100, 101) == 100
assert quote("sell", "stop", 100, 101, 102) is None

# Gap: a stop fills at the executable quote, not at its level.
assert quote("buy", "stop", 100, 104, 105) == 105
assert quote("sell", "stop", 100, 95, 96) == 95

# Improvement: limits fill at the better executable quote.
assert quote("buy", "limit", 100, 97, 98) == 98
assert quote("sell", "limit", 100, 103, 104) == 103
assert quote("buy", "limit", 100, 100, 101) is None

# Adverse slippage: market and stop fills only.
assert quote("buy", "market", None, 99, 100, 2) == 102
assert quote("sell", "market", None, 99, 100, 2) == 97
assert quote("buy", "stop", 100, 99, 100, 3) == 103
assert quote("buy", "limit", 100, 97, 98, 3) == 98
assert quote("sell", "limit", 100, 103, 104, 3) == 103

# Large valid prices and minimum size.
big = 2**63 // 4
assert quote("sell", "stop", big, big, big + 1) == big
assert trade("sell", "stop", big, big) == big
assert quote("buy", "market", None, 1, 1) == 1
assert trade("buy", "stop", 1, 1) == 1
assert quote("buy", "market", None, 2**63 - 2, 2**63 - 2, 1) == 2**63 - 1

# Repeated calls are identical.
first = quote("buy", "stop", 100, 99, 105, 2)
assert all(quote("buy", "stop", 100, 99, 105, 2) == first for _ in range(1000))

# Overflow and invalid inputs.
raises(OverflowError, quote, "buy", "market", None, 2**63 - 2, 2**63 - 2, 2)
raises(OverflowError, quote, "buy", "market", None, 99, 100, 2**63)
raises(OverflowError, trade, "buy", "stop", 2**63, 100)
raises(ValueError, quote, "buy", "market", None, 101, 100)
raises(ValueError, quote, "buy", "market", None, 0, 100)
raises(ValueError, quote, "buy", "market", None, 99, 100, -1)
raises(ValueError, quote, "buy", "market", 100, 99, 100)
raises(ValueError, quote, "buy", "stop", None, 99, 100)
raises(ValueError, quote, "buy", "limit", 0, 99, 100)
raises(ValueError, quote, "sell", "market", None, 3, 4, 3)
raises(ValueError, quote, "long", "market", None, 99, 100)
raises(ValueError, quote, "buy", "ioc", 100, 99, 100)
raises(ValueError, trade, "buy", "market", 100, 100)
raises(ValueError, trade, "buy", "limit", 0, 100)
raises(TypeError, quote, "buy", "limit", 100.0, 99, 100)

# Fill cost: per-contract and basis-point fees, rounded once.
assert fee() == "0.04"
assert fee(currency_decimals=4) == "0.0420"
assert fee(cost_per_contract="0", cost_bps="0") == "0.00"
assert fee(quantity_steps=1, volume_step="1", price_ticks=1, tick_size="1", point_value="1",
           cost_per_contract="0.125", cost_bps="0", currency_decimals=2) == "0.12"
assert fee(quantity_steps=1, volume_step="1", price_ticks=1, tick_size="1", point_value="1",
           cost_per_contract="0.135", cost_bps="0", currency_decimals=2) == "0.14"
assert fee(quantity_steps=1, volume_step="1", price_ticks=1, tick_size="1", point_value="1",
           cost_per_contract="0.145", cost_bps="0", currency_decimals=2) == "0.14"
raises(ValueError, fee, cost_per_contract="-1")
raises(ValueError, fee, quantity_steps=-1)
raises(ValueError, fee, tick_size="0")
raises(ValueError, fee, volume_step="1.2.3")
raises(ValueError, fee, currency_decimals=29)
raises(ValueError, fee, currency_decimals=-1)
raises(TypeError, fee, volume_step=0.01)
raises(OverflowError, fee, quantity_steps=2**63)
raises(OverflowError, fee, volume_step="0.0000000000000000000000000001",
       tick_size="0.0000000000000000000000000001", quantity_steps=1, price_ticks=1,
       point_value="1", cost_per_contract="1", cost_bps="0", currency_decimals=2)

# Price to ticks: on-grid conversion, off-grid rejection.
assert ex.price_to_ticks(price="100.25", tick_size="0.25") == 401
assert ex.price_to_ticks(price="5", tick_size="0.5") == 10
raises(ValueError, ex.price_to_ticks, price="100.10", tick_size="0.25")
raises(ValueError, ex.price_to_ticks, price="0", tick_size="0.25")

print("all incremental execution cases ok")
"""


if __name__ == "__main__":
    main()
