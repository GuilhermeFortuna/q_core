"""Reference fixture exporter and provenance verifier."""

from __future__ import annotations

import importlib.metadata
import subprocess
import sys
import tomllib
from dataclasses import dataclass
from pathlib import Path
from typing import Final

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
