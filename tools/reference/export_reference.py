"""Reference fixture exporter and provenance verifier."""

from __future__ import annotations

import ast
import importlib.metadata
import importlib.util
import subprocess
import sys
import tomllib
from collections.abc import Callable
from dataclasses import dataclass
from pathlib import Path
from types import ModuleType
from typing import Final

import numpy as np
import pandas as pd

FORMAT: Final = "q-core-reference-fixture/1"
ABS_TOL: Final = 1e-10
REL_TOL: Final = 1e-12
CANONICAL_NAN_BITS: Final = 0x7FF8000000000000
LOCKED_PACKAGES: Final = ("numpy", "pandas", "python-dateutil", "tzdata")


@dataclass(frozen=True)
class BackendSource:
    checkout: Path          # a checkout whose HEAD is rev
    rev: str                # 40-hex commit
    repo_url: str


def fetch_backend(repo_url: str, rev: str, workdir: Path) -> BackendSource:
    """Fetch backend commit shallowly by hash into workdir."""
    workdir.mkdir(parents=True, exist_ok=True)
    subprocess.run(["git", "init"], cwd=workdir, check=True, capture_output=True)
    subprocess.run(["git", "fetch", "--depth", "1", repo_url, rev], cwd=workdir, check=True, capture_output=True)
    subprocess.run(["git", "checkout", "--detach", "FETCH_HEAD"], cwd=workdir, check=True, capture_output=True)
    return BackendSource(checkout=workdir, rev=rev, repo_url=repo_url)


def open_backend_checkout(path: Path, rev: str) -> BackendSource:
    """Validate that path is a clean git checkout at rev."""
    try:
        res = subprocess.run(["git", "-C", str(path), "rev-parse", "HEAD"], check=True, capture_output=True, text=True)
        head = res.stdout.strip()
    except subprocess.CalledProcessError:
        sys.stderr.write(f"Failed to get HEAD from {path}\n")
        sys.exit(2)
    if head != rev:
        sys.stderr.write(f"Checkout HEAD {head} does not match expected rev {rev}\n")
        sys.exit(2)
    try:
        status = subprocess.run(["git", "-C", str(path), "status", "--porcelain"], check=True, capture_output=True, text=True)
        if status.stdout.strip():
            sys.stderr.write(f"Checkout at {path} is not clean\n")
            sys.exit(2)
    except subprocess.CalledProcessError:
        sys.stderr.write(f"Failed to check git status in {path}\n")
        sys.exit(2)
    try:
        url_res = subprocess.run(["git", "-C", str(path), "config", "--get", "remote.origin.url"], capture_output=True, text=True)
        repo_url = url_res.stdout.strip() or "https://github.com/GuilhermeFortuna/q_backend.git"
    except Exception:
        repo_url = "https://github.com/GuilhermeFortuna/q_backend.git"
    return BackendSource(checkout=path, rev=rev, repo_url=repo_url)


def git_blob_id(source: BackendSource, rel_path: str) -> str:
    """Return the git blob object id of a file at HEAD."""
    res = subprocess.run(["git", "-C", str(source.checkout), "rev-parse", f"HEAD:{rel_path}"], check=True, capture_output=True, text=True)
    return res.stdout.strip()


def verify_environment(source: BackendSource) -> dict[str, str]:
    """Verify that installed packages match backend uv.lock exact versions."""
    lock_file = source.checkout / "uv.lock"
    if not lock_file.exists():
        sys.stderr.write(f"uv.lock not found in {source.checkout}\n")
        sys.exit(2)
    try:
        lock_data = tomllib.loads(lock_file.read_text(encoding="utf-8"))
    except Exception as e:
        sys.stderr.write(f"Failed to parse uv.lock: {e}\n")
        sys.exit(2)
    packages: dict[str, str] = {}
    for pkg in lock_data.get("package", []):
        name = pkg.get("name")
        version = pkg.get("version")
        if name and version:
            packages[name] = version

    result: dict[str, str] = {}
    for pkg_name in LOCKED_PACKAGES:
        if pkg_name not in packages:
            sys.stderr.write(f"Package {pkg_name} not found in backend uv.lock\n")
            sys.exit(2)
        expected_ver = packages[pkg_name]
        try:
            installed_ver = importlib.metadata.version(pkg_name)
        except importlib.metadata.PackageNotFoundError:
            sys.stderr.write(f"Package {pkg_name} is not installed\n")
            sys.exit(2)
        if installed_ver != expected_ver:
            sys.stderr.write(f"Environment mismatch for package {pkg_name}: installed {installed_ver} != locked {expected_ver}\n")
            sys.exit(2)
        result[pkg_name] = installed_ver
    return result


def _is_type_checking_guard(test_node: ast.expr) -> bool:
    if isinstance(test_node, ast.Name) and test_node.id == "TYPE_CHECKING":
        return True
    if isinstance(test_node, ast.Attribute) and test_node.attr == "TYPE_CHECKING":
        return True
    return False


def check_imports(module_source: str, rel_path: str) -> None:
    """Verify that module imports only stdlib, numpy, and pandas outside TYPE_CHECKING."""
    tree = ast.parse(module_source, filename=rel_path)
    allowed = set(sys.stdlib_module_names) | {"numpy", "pandas"}

    def walk_stmts(stmts: list[ast.stmt]) -> None:
        for stmt in stmts:
            if isinstance(stmt, ast.If):
                if _is_type_checking_guard(stmt.test):
                    walk_stmts(stmt.orelse)
                else:
                    walk_stmts(stmt.body)
                    walk_stmts(stmt.orelse)
            elif isinstance(stmt, ast.Try):
                walk_stmts(stmt.body)
                for handler in stmt.handlers:
                    walk_stmts(handler.body)
                walk_stmts(stmt.orelse)
                walk_stmts(stmt.finalbody)
            elif isinstance(stmt, ast.Import):
                for alias in stmt.names:
                    top = alias.name.split(".")[0]
                    if top not in allowed:
                        raise ValueError(f"Disallowed import '{alias.name}' in {rel_path}")
            elif isinstance(stmt, ast.ImportFrom):
                if stmt.level == 0 and stmt.module:
                    top = stmt.module.split(".")[0]
                    if top not in allowed:
                        raise ValueError(f"Disallowed import from '{stmt.module}' in {rel_path}")
                elif stmt.level > 0:
                    raise ValueError(f"Disallowed relative import in {rel_path}")

    walk_stmts(tree.body)


def load_reference_module(source: BackendSource, rel_path: str) -> ModuleType:
    """Load a reference module by file path after verifying its imports."""
    file_path = source.checkout / rel_path
    source_code = file_path.read_text(encoding="utf-8")
    check_imports(source_code, rel_path)
    module_name = f"_reference_{Path(rel_path).stem}"
    spec = importlib.util.spec_from_file_location(module_name, file_path)
    if spec is None or spec.loader is None:
        raise ImportError(f"Could not create spec for {file_path}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[module_name] = module
    spec.loader.exec_module(module)
    return module


def load_generator(source: BackendSource, rel_path: str, name: str) -> Callable[..., pd.DataFrame]:
    """Extract a generator function by AST and execute in a namespace with only np and pd."""
    file_path = source.checkout / rel_path
    content = file_path.read_text(encoding="utf-8")
    tree = ast.parse(content, filename=rel_path)
    fn_node: ast.FunctionDef | None = None
    for node in tree.body:
        if isinstance(node, ast.FunctionDef) and node.name == name:
            fn_node = node
            break
    if fn_node is None:
        raise ValueError(f"Function '{name}' not found in {rel_path}")
    mod = ast.Module(body=[fn_node], type_ignores=[])
    ast.fix_missing_locations(mod)
    code = compile(mod, filename=rel_path, mode="exec")
    ns: dict[str, object] = {"np": np, "pd": pd}
    exec(code, ns)
    fn = ns[name]
    if not callable(fn):
        raise TypeError(f"Extracted object '{name}' is not callable")
    return fn
