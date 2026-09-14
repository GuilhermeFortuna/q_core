from __future__ import annotations

import io
import json
import struct
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

from export_reference import (
    FORMAT,
    ABS_TOL,
    REL_TOL,
    CANONICAL_NAN_BITS,
    LOCKED_PACKAGES,
    BackendSource,
    FunctionSpec,
    build_inputs,
    check_imports,
    check_reference_causal,
    compare_trees,
    cpu_feature_level,
    dumps_fixture,
    encode_column,
    fetch_backend,
    fnv1a64_float64,
    function_specs,
    git_blob_id,
    load_generator,
    load_reference_module,
    open_backend_checkout,
    run_case,
    verify_environment,
)
import numpy as np


def get_or_fetch_backend() -> BackendSource:
    rev = (Path(__file__).resolve().parents[2] / "BACKEND_REV").read_text().strip()
    sibling = Path(__file__).resolve().parents[3] / "q_backend"
    if sibling.exists():
        try:
            head = subprocess.run(
                ["git", "-C", str(sibling), "rev-parse", "HEAD"],
                capture_output=True,
                text=True,
                check=True,
            ).stdout.strip()
            if head == rev:
                return BackendSource(
                    checkout=sibling,
                    rev=rev,
                    repo_url="https://github.com/GuilhermeFortuna/q_backend.git",
                )
        except Exception:
            pass
    cache_dir = Path(tempfile.gettempdir()) / f"q_backend_{rev}"
    if not cache_dir.exists() or not (cache_dir / "tests/backtesting/test_goldens.py").exists():
        fetch_backend("https://github.com/GuilhermeFortuna/q_backend.git", rev, cache_dir)
    return BackendSource(
        checkout=cache_dir,
        rev=rev,
        repo_url="https://github.com/GuilhermeFortuna/q_backend.git",
    )


class TestStep2EnvironmentAndCheckout(unittest.TestCase):
    def test_constants(self) -> None:
        self.assertEqual(FORMAT, "q-core-reference-fixture/1")
        self.assertEqual(ABS_TOL, 1e-10)
        self.assertEqual(REL_TOL, 1e-12)
        self.assertEqual(CANONICAL_NAN_BITS, 0x7FF8000000000000)
        self.assertEqual(LOCKED_PACKAGES, ("numpy", "pandas", "python-dateutil", "tzdata"))

    def test_verify_environment_mismatch_exits_2(self) -> None:
        with tempfile.TemporaryDirectory() as tmpdir:
            tmp_path = Path(tmpdir)
            # Create uv.lock with pandas 3.0.1 (installed is 3.0.2)
            fake_lock = """
[[package]]
name = "numpy"
version = "2.4.4"

[[package]]
name = "pandas"
version = "3.0.1"

[[package]]
name = "python-dateutil"
version = "2.9.0.post0"

[[package]]
name = "tzdata"
version = "2026.2"
"""
            (tmp_path / "uv.lock").write_text(fake_lock, encoding="utf-8")
            source = BackendSource(checkout=tmp_path, rev="0" * 40, repo_url="https://example.com/repo.git")

            stderr_buf = io.StringIO()
            with self.assertRaises(SystemExit) as cm:
                orig_stderr = sys.stderr
                sys.stderr = stderr_buf
                try:
                    verify_environment(source)
                finally:
                    sys.stderr = orig_stderr
            self.assertEqual(cm.exception.code, 2)
            self.assertIn("pandas", stderr_buf.getvalue())

    def test_verify_environment_matching_returns_four_versions(self) -> None:
        with tempfile.TemporaryDirectory() as tmpdir:
            tmp_path = Path(tmpdir)
            fake_lock = """
[[package]]
name = "numpy"
version = "2.4.4"

[[package]]
name = "pandas"
version = "3.0.2"

[[package]]
name = "python-dateutil"
version = "2.9.0.post0"

[[package]]
name = "tzdata"
version = "2026.2"
"""
            (tmp_path / "uv.lock").write_text(fake_lock, encoding="utf-8")
            source = BackendSource(checkout=tmp_path, rev="0" * 40, repo_url="https://example.com/repo.git")
            versions = verify_environment(source)
            self.assertEqual(
                versions,
                {
                    "numpy": "2.4.4",
                    "pandas": "3.0.2",
                    "python-dateutil": "2.9.0.post0",
                    "tzdata": "2026.2",
                },
            )

    def test_open_backend_checkout_wrong_head_exits_2(self) -> None:
        with tempfile.TemporaryDirectory() as tmpdir:
            tmp_path = Path(tmpdir)
            subprocess.run(["git", "init"], cwd=tmp_path, check=True, capture_output=True)
            subprocess.run(["git", "config", "user.name", "Test"], cwd=tmp_path, check=True, capture_output=True)
            subprocess.run(["git", "config", "user.email", "test@test.com"], cwd=tmp_path, check=True, capture_output=True)
            (tmp_path / "test.txt").write_text("hello", encoding="utf-8")
            subprocess.run(["git", "add", "."], cwd=tmp_path, check=True, capture_output=True)
            subprocess.run(["git", "commit", "-m", "init"], cwd=tmp_path, check=True, capture_output=True)

            stderr_buf = io.StringIO()
            with self.assertRaises(SystemExit) as cm:
                orig_stderr = sys.stderr
                sys.stderr = stderr_buf
                try:
                    open_backend_checkout(tmp_path, "1" * 40)
                finally:
                    sys.stderr = orig_stderr
            self.assertEqual(cm.exception.code, 2)

    def test_open_backend_checkout_dirty_tree_exits_2(self) -> None:
        with tempfile.TemporaryDirectory() as tmpdir:
            tmp_path = Path(tmpdir)
            subprocess.run(["git", "init"], cwd=tmp_path, check=True, capture_output=True)
            subprocess.run(["git", "config", "user.name", "Test"], cwd=tmp_path, check=True, capture_output=True)
            subprocess.run(["git", "config", "user.email", "test@test.com"], cwd=tmp_path, check=True, capture_output=True)
            (tmp_path / "test.txt").write_text("hello", encoding="utf-8")
            subprocess.run(["git", "add", "."], cwd=tmp_path, check=True, capture_output=True)
            subprocess.run(["git", "commit", "-m", "init"], cwd=tmp_path, check=True, capture_output=True)
            rev = subprocess.run(["git", "rev-parse", "HEAD"], cwd=tmp_path, check=True, capture_output=True, text=True).stdout.strip()

            # Make tree dirty
            (tmp_path / "dirty.txt").write_text("untracked", encoding="utf-8")

            stderr_buf = io.StringIO()
            with self.assertRaises(SystemExit) as cm:
                orig_stderr = sys.stderr
                sys.stderr = stderr_buf
                try:
                    open_backend_checkout(tmp_path, rev)
                finally:
                    sys.stderr = orig_stderr
            self.assertEqual(cm.exception.code, 2)

    def test_open_backend_checkout_clean_and_matching(self) -> None:
        with tempfile.TemporaryDirectory() as tmpdir:
            tmp_path = Path(tmpdir)
            subprocess.run(["git", "init"], cwd=tmp_path, check=True, capture_output=True)
            subprocess.run(["git", "config", "user.name", "Test"], cwd=tmp_path, check=True, capture_output=True)
            subprocess.run(["git", "config", "user.email", "test@test.com"], cwd=tmp_path, check=True, capture_output=True)
            (tmp_path / "test.txt").write_text("hello", encoding="utf-8")
            subprocess.run(["git", "add", "."], cwd=tmp_path, check=True, capture_output=True)
            subprocess.run(["git", "commit", "-m", "init"], cwd=tmp_path, check=True, capture_output=True)
            rev = subprocess.run(["git", "rev-parse", "HEAD"], cwd=tmp_path, check=True, capture_output=True, text=True).stdout.strip()

            source = open_backend_checkout(tmp_path, rev)
            self.assertEqual(source.checkout, tmp_path)
            self.assertEqual(source.rev, rev)

    def test_git_blob_id(self) -> None:
        with tempfile.TemporaryDirectory() as tmpdir:
            tmp_path = Path(tmpdir)
            subprocess.run(["git", "init"], cwd=tmp_path, check=True, capture_output=True)
            subprocess.run(["git", "config", "user.name", "Test"], cwd=tmp_path, check=True, capture_output=True)
            subprocess.run(["git", "config", "user.email", "test@test.com"], cwd=tmp_path, check=True, capture_output=True)
            (tmp_path / "foo.txt").write_text("sample content\n", encoding="utf-8")
            subprocess.run(["git", "add", "."], cwd=tmp_path, check=True, capture_output=True)
            subprocess.run(["git", "commit", "-m", "init"], cwd=tmp_path, check=True, capture_output=True)
            rev = subprocess.run(["git", "rev-parse", "HEAD"], cwd=tmp_path, check=True, capture_output=True, text=True).stdout.strip()
            source = BackendSource(checkout=tmp_path, rev=rev, repo_url="https://example.com/repo.git")

            expected_blob = subprocess.run(
                ["git", "rev-parse", "HEAD:foo.txt"], cwd=tmp_path, check=True, capture_output=True, text=True
            ).stdout.strip()
            self.assertEqual(git_blob_id(source, "foo.txt"), expected_blob)



class TestStep3ImportsAndGenerator(unittest.TestCase):
    def setUp(self) -> None:
        self.source = get_or_fetch_backend()

    def test_check_imports_accepts_reference_files(self) -> None:
        files = [
            "src/q_backend/backtesting/technical_indicators.py",
            "src/q_backend/backtesting/moving_averages.py",
            "src/q_backend/backtesting/transforms.py",
            "src/q_backend/features/leakage.py",
        ]
        for rel_path in files:
            content = (self.source.checkout / rel_path).read_text(encoding="utf-8")
            # Should not raise
            check_imports(content, rel_path)

    def test_check_imports_rejects_disallowed(self) -> None:
        with self.assertRaises(ValueError):
            check_imports("import sqlalchemy", "test.py")
        with self.assertRaises(ValueError):
            check_imports("from q_core import compute_rsi", "test.py")
        with self.assertRaises(ValueError):
            check_imports("import torch\nimport numpy as np", "test.py")

    def test_check_imports_accepts_disallowed_inside_type_checking(self) -> None:
        source_code = """
from typing import TYPE_CHECKING
if TYPE_CHECKING:
    import sqlalchemy
    from q_core import compute_rsi
"""
        # Should not raise
        check_imports(source_code, "test.py")

    def test_load_generator_synthetic_ohlcv(self) -> None:
        gen = load_generator(self.source, "tests/backtesting/test_goldens.py", "synthetic_ohlcv")
        df = gen()
        self.assertEqual(df.shape, (400, 5))
        self.assertEqual(list(df.columns), ["open", "high", "low", "close", "volume"])
        expected_close0 = 100.0 + np.random.default_rng(20240609).normal(0.0, 1.0, 400)[0]
        self.assertEqual(df["close"].iloc[0], expected_close0)

    def test_load_generator_undefined_name_raises(self) -> None:
        with tempfile.TemporaryDirectory() as tmpdir:
            tmp_path = Path(tmpdir)
            source = BackendSource(checkout=tmp_path, rev="0" * 40, repo_url="https://example.com")
            (tmp_path / "bad.py").write_text("def bad_gen(): return undefined_name\n", encoding="utf-8")
            fn = load_generator(source, "bad.py", "bad_gen")
            with self.assertRaises(NameError):
                fn()

            (tmp_path / "bad_sig.py").write_text("def bad_sig(x=undefined_name): pass\n", encoding="utf-8")
            with self.assertRaises(NameError):
                load_generator(source, "bad_sig.py", "bad_sig")


import math


class TestStep4EncodingAndProvenance(unittest.TestCase):
    def test_cpu_feature_level_fake_cpuinfo(self) -> None:
        v4_cpuinfo = (
            "flags : cx16 lahf_lm popcnt ssse3 sse4_1 sse4_2 avx avx2 bmi1 bmi2 f16c fma movbe abm "
            "avx512f avx512bw avx512cd avx512dq avx512vl"
        )
        self.assertEqual(cpu_feature_level(v4_cpuinfo), "x86-64-v4")

        v3_cpuinfo = "flags : cx16 lahf_lm popcnt ssse3 sse4_1 sse4_2 avx avx2 bmi1 bmi2 f16c fma movbe abm"
        self.assertEqual(cpu_feature_level(v3_cpuinfo), "x86-64-v3")

        v2_cpuinfo = "flags : cx16 lahf_lm popcnt ssse3 sse4_1 sse4_2"
        self.assertEqual(cpu_feature_level(v2_cpuinfo), "x86-64-v2")

    def test_fnv1a64_float64_known_literals(self) -> None:
        self.assertEqual(fnv1a64_float64([]), 0xCBF29CE484222325)
        self.assertEqual(
            fnv1a64_float64([1.0, -0.0, float("nan"), float("inf")]),
            0x892EAB94389F5CB0,
        )
        nan_alt = struct.unpack("<d", struct.pack("<Q", 0xFFF8000000000000))[0]
        self.assertEqual(
            fnv1a64_float64([nan_alt]),
            fnv1a64_float64([float("nan")]),
        )

    def test_encode_column(self) -> None:
        raw_vals = [float("nan"), float("inf"), float("-inf"), -0.0, 100.5]
        col = encode_column(np.array(raw_vals))
        self.assertEqual(col["dtype"], "float64")
        self.assertIsNone(col["values"][0])
        self.assertEqual(col["values"][1], "inf")
        self.assertEqual(col["values"][2], "-inf")
        self.assertEqual(col["values"][3], -0.0)
        self.assertTrue(math.copysign(1.0, col["values"][3]) < 0)
        self.assertEqual(col["values"][4], 100.5)
        self.assertEqual(
            col["bits_fnv1a64"],
            f"0x{fnv1a64_float64(raw_vals):016x}",
        )

    def test_dumps_fixture(self) -> None:
        payload = {"b": 2, "a": [1.0, None, -0.0, "inf"]}
        s1 = dumps_fixture(payload)
        s2 = dumps_fixture(payload)
        self.assertEqual(s1, s2)
        self.assertTrue(s1.endswith("\n"))
        # Verify keys sorted
        self.assertTrue(s1.index('"a"') < s1.index('"b"'))

        # Roundtrip preservation of finite float bits
        finite_val = 12345.678901234567
        s = dumps_fixture({"val": finite_val})
        loaded = json.loads(s)
        self.assertEqual(
            struct.pack("<d", finite_val),
            struct.pack("<d", loaded["val"]),
        )

        # Raw NaN should raise ValueError
        with self.assertRaises(ValueError):
            dumps_fixture({"raw_nan": float("nan")})

    def test_compare_trees(self) -> None:
        with tempfile.TemporaryDirectory() as tmpdir:
            tmp = Path(tmpdir)
            committed = tmp / "committed"
            regen = tmp / "regen"
            (committed / "indicators").mkdir(parents=True)
            (committed / "inputs").mkdir(parents=True)
            (regen / "indicators").mkdir(parents=True)
            (regen / "inputs").mkdir(parents=True)

            val_orig = 100.0
            bits_orig = struct.unpack("<Q", struct.pack("<d", val_orig))[0]
            val_ulp = struct.unpack("<d", struct.pack("<Q", bits_orig + 1))[0]

            def make_input(val: float, rev: str = "0" * 40) -> dict[str, object]:
                return {
                    "format": "q-core-reference-fixture/1",
                    "input_id": "test_input",
                    "provenance": {
                        "backend_repo": "https://example.com/repo.git",
                        "backend_rev": rev,
                        "exporter": "tools/reference/export_reference.py",
                    },
                    "columns": {
                        "close": encode_column([val]),
                    },
                }

            def make_fixture(val: float, cpu: str = "x86-64-v3", rev: str = "0" * 40) -> dict[str, object]:
                return {
                    "format": "q-core-reference-fixture/1",
                    "family": "indicators",
                    "fixture_id": "test_fn",
                    "policy": {"kind": "abs_rel_tol", "abs": 1e-10, "rel": 1e-12},
                    "provenance": {
                        "backend_repo": "https://example.com/repo.git",
                        "backend_rev": rev,
                        "cpu_level": cpu,
                        "environment": "numeric",
                    },
                    "cases": [
                        {
                            "case_id": "c1",
                            "inputs": {"close": "test_input.close"},
                            "params": {"period": 14},
                            "expected": {
                                "outputs": {
                                    "rsi": encode_column([val]),
                                }
                            },
                        }
                    ],
                }

            # 1. Output differs by 1 ULP, same cpu_level -> fails (returns 1)
            (committed / "inputs/test_input.json").write_text(dumps_fixture(make_input(val_orig)))
            (regen / "inputs/test_input.json").write_text(dumps_fixture(make_input(val_orig)))
            (committed / "indicators/test_fn.json").write_text(dumps_fixture(make_fixture(val_orig, cpu="x86-64-v3")))
            (regen / "indicators/test_fn.json").write_text(dumps_fixture(make_fixture(val_ulp, cpu="x86-64-v3")))

            self.assertEqual(compare_trees(committed, regen), 1)

            # 2. Output differs by 1 ULP, different cpu_level -> passes (returns 0)
            (regen / "indicators/test_fn.json").write_text(dumps_fixture(make_fixture(val_ulp, cpu="x86-64-v4")))
            self.assertEqual(compare_trees(committed, regen), 0)

            # 3. Input differs by 1 ULP, different cpu_level -> fails (returns 1)
            (regen / "inputs/test_input.json").write_text(dumps_fixture(make_input(val_ulp)))
            self.assertEqual(compare_trees(committed, regen), 1)

            # Revert input
            (regen / "inputs/test_input.json").write_text(dumps_fixture(make_input(val_orig)))
            self.assertEqual(compare_trees(committed, regen), 0)

            # 4. backend_rev differs, different cpu_level -> fails (returns 1)
            (regen / "indicators/test_fn.json").write_text(dumps_fixture(make_fixture(val_ulp, cpu="x86-64-v4", rev="1" * 40)))
class TestStep5Cases(unittest.TestCase):
    def setUp(self) -> None:
        self.source = get_or_fetch_backend()
        generator = load_generator(self.source, "tests/backtesting/test_goldens.py", "synthetic_ohlcv")
        self.inputs = build_inputs(generator)
        self.tech_mod = load_reference_module(self.source, "src/q_backend/backtesting/technical_indicators.py")
        self.ma_mod = load_reference_module(self.source, "src/q_backend/backtesting/moving_averages.py")
        self.trans_mod = load_reference_module(self.source, "src/q_backend/backtesting/transforms.py")
        self.leak_mod = load_reference_module(self.source, "src/q_backend/features/leakage.py")

    def test_function_specs_count_and_ids(self) -> None:
        specs = function_specs()
        self.assertEqual(len(specs), 16)
        expected_ids = {
            "realized_vol",
            "yang_zhang",
            "rsi",
            "bollinger_bands",
            "macd",
            "donchian_channels",
            "atr",
            "ma_sma",
            "ma_ema",
            "ma_smma",
            "ma_wma",
            "ma_hma",
            "rolling_zscore",
            "rolling_rank",
            "pct_change",
            "clip",
        }
        self.assertEqual(set(s.function_id for s in specs), expected_ids)

    def test_run_case_rsi(self) -> None:
        rsi_spec = next(s for s in function_specs() if s.function_id == "rsi")

        # 1. On monotonic_up_n60 with period 14: NaN at 0..13, 100.0 at 14
        res_up = run_case(rsi_spec, self.tech_mod, self.inputs["monotonic_up_n60"], {"period": 14})
        self.assertIn("outputs", res_up)
        vals_up = res_up["outputs"]["rsi"]["values"]
        for idx in range(14):
            self.assertIsNone(vals_up[idx], f"Expected NaN at index {idx}")
        self.assertEqual(vals_up[14], 100.0)

        # 2. On constant_n60: all NaN
        res_const = run_case(rsi_spec, self.tech_mod, self.inputs["constant_n60"], {"period": 14})
        self.assertIn("outputs", res_const)
        vals_const = res_const["outputs"]["rsi"]["values"]
        for idx, v in enumerate(vals_const):
            self.assertIsNone(v, f"Expected NaN at index {idx}")

        # 3. Period 0 gives ZeroDivisionError
        res_p0 = run_case(rsi_spec, self.tech_mod, self.inputs["monotonic_up_n60"], {"period": 0})
        self.assertEqual(res_p0, {"rejected": {"python_exception": "ZeroDivisionError"}})

    def test_run_case_rolling_rank(self) -> None:
        rank_spec = next(s for s in function_specs() if s.function_id == "rolling_rank")
        res = run_case(rank_spec, self.trans_mod, self.inputs["synthetic_ohlcv_n400"], {"window": 0})
        self.assertEqual(res, {"rejected": {"python_exception": "IndexError"}})

    def test_run_case_donchian_channels(self) -> None:
        donchian_spec = next(s for s in function_specs() if s.function_id == "donchian_channels")
        res = run_case(donchian_spec, self.tech_mod, self.inputs["synthetic_ohlcv_n400"], {"period": 0})
        self.assertIn("outputs", res)
        self.assertIn("upper", res["outputs"])
        self.assertIn("lower", res["outputs"])
        for v in res["outputs"]["upper"]["values"]:
            self.assertIsNone(v)
        for v in res["outputs"]["lower"]["values"]:
            self.assertIsNone(v)

    def test_run_case_bollinger_bands(self) -> None:
        bb_spec = next(s for s in function_specs() if s.function_id == "bollinger_bands")
        res = run_case(bb_spec, self.tech_mod, self.inputs["synthetic_ohlcv_n400"], {"period": 20, "num_std": 2.0})
        self.assertIn("outputs", res)
        self.assertEqual(set(res["outputs"].keys()), {"upper", "middle", "lower"})

    def test_check_reference_causal_raises_leakage_error(self) -> None:
        class DummyLeakyModule:
            @staticmethod
            def leaky_fn(close: pd.Series) -> pd.Series:
                return close.shift(-1)

        leaky_spec = FunctionSpec(
            function_id="test_leaky",
            module_path="dummy.py",
            callable_name="leaky_fn",
            input_columns=("close",),
            outputs=("out",),
            param_grid=({},),
            fixed_kwargs={},
        )
        with self.assertRaises(self.leak_mod.LeakageError):
            check_reference_causal(
                leaky_spec,
                DummyLeakyModule(),  # type: ignore[arg-type]
                self.inputs["synthetic_ohlcv_n400"],
                {},
                self.leak_mod,
            )


if __name__ == "__main__":
    unittest.main()
