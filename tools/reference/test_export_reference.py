from __future__ import annotations

import io
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
    check_imports,
    fetch_backend,
    git_blob_id,
    load_generator,
    load_reference_module,
    open_backend_checkout,
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


if __name__ == "__main__":
    unittest.main()
