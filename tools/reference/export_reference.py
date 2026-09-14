"""Reference fixture exporter and provenance verifier."""

from __future__ import annotations

import argparse
import ast
import importlib.metadata
import importlib.util
import json
import math
import platform
import struct
import subprocess
import sys
import tempfile
import tomllib
from collections.abc import Callable
from dataclasses import dataclass
from pathlib import Path
from types import ModuleType
from typing import Final, Literal, Protocol

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


def cpu_feature_level(cpuinfo_text: str | None = None) -> str:
    """Return the highest x86-64 psABI level (v2, v3, v4) or platform.machine()."""
    mach = platform.machine()
    if cpuinfo_text is None:
        if mach not in ("x86_64", "AMD64", "x86-64"):
            return mach
        cpuinfo_path = Path("/proc/cpuinfo")
        if not cpuinfo_path.exists():
            return mach
        try:
            cpuinfo_text = cpuinfo_path.read_text(encoding="utf-8")
        except Exception:
            return mach

    flags: set[str] = set()
    for line in cpuinfo_text.splitlines():
        if line.startswith("flags") or line.startswith("Features"):
            parts = line.split(":", 1)
            if len(parts) == 2:
                flags.update(parts[1].strip().split())

    v2_flags = {"cx16", "lahf_lm", "popcnt", "ssse3", "sse4_1", "sse4_2"}
    v3_flags = {"avx", "avx2", "bmi1", "bmi2", "f16c", "fma", "movbe"}
    v4_flags = {"avx512f", "avx512bw", "avx512cd", "avx512dq", "avx512vl"}

    if (flags & v4_flags) == v4_flags and (flags & v3_flags) == v3_flags and (flags & v2_flags) == v2_flags:
        return "x86-64-v4"
    if (flags & v3_flags) == v3_flags and (flags & v2_flags) == v2_flags:
        return "x86-64-v3"
    if (flags & v2_flags) == v2_flags:
        return "x86-64-v2"
    return mach if mach not in ("x86_64", "AMD64", "x86-64") else "x86-64"


FNV_OFFSET: Final = 0xCBF29CE484222325
FNV_PRIME: Final = 0x100000001B3
CANONICAL_NAN_BYTES: Final = struct.pack("<Q", CANONICAL_NAN_BITS)


def fnv1a64_float64(values: np.ndarray | list[float]) -> int:
    """Compute FNV-1a 64-bit checksum over little-endian float64 values with NaN canonicalisation."""
    h = FNV_OFFSET
    for v in values:
        fv = float(v)
        if math.isnan(fv):
            raw = CANONICAL_NAN_BYTES
        else:
            raw = struct.pack("<d", fv)
        for b in raw:
            h = ((h ^ b) * FNV_PRIME) & 0xFFFFFFFFFFFFFFFF
    return h


def fnv1a64_int64(values: np.ndarray | list[int]) -> int:
    """Compute FNV-1a 64-bit checksum over little-endian int64 values."""
    h = FNV_OFFSET
    for v in values:
        raw = struct.pack("<q", int(v))
        for b in raw:
            h = ((h ^ b) * FNV_PRIME) & 0xFFFFFFFFFFFFFFFF
    return h


def encode_column(values: np.ndarray | list[object]) -> dict[str, object]:
    """Encode a series into a JSON-friendly column dictionary with verified FNV-1a checksum."""
    arr = np.asarray(values)
    if np.issubdtype(arr.dtype, np.integer):
        h = fnv1a64_int64(arr)
        return {
            "bits_fnv1a64": f"0x{h:016x}",
            "dtype": "int64",
            "values": [int(x) for x in arr],
        }
    float_arr = arr.astype(np.float64, copy=False)
    h = fnv1a64_float64(float_arr)
    encoded: list[object] = []
    for v in float_arr:
        fv = float(v)
        if math.isnan(fv):
            encoded.append(None)
        elif math.isinf(fv):
            encoded.append("inf" if fv > 0 else "-inf")
        elif fv == 0.0 and math.copysign(1.0, fv) < 0.0:
            encoded.append(-0.0)
        else:
            encoded.append(fv)
    return {
        "bits_fnv1a64": f"0x{h:016x}",
        "dtype": "float64",
        "values": encoded,
    }


def dumps_fixture(payload: dict[str, object]) -> str:
    """Dump fixture dict to deterministic canonical JSON ending in newline."""
    return json.dumps(payload, indent=1, sort_keys=True, allow_nan=False) + "\n"


def _compare_column_values(
    comm_col: dict[str, object],
    regen_col: dict[str, object],
    policy: dict[str, object],
    context: str,
) -> bool:
    c_vals = comm_col.get("values", [])
    r_vals = regen_col.get("values", [])
    if len(c_vals) != len(r_vals):
        sys.stderr.write(f"Length mismatch in {context}: expected {len(c_vals)}, got {len(r_vals)}\n")
        return False
    kind = policy.get("kind", "exact")
    if kind == "exact":
        for idx, (e, a) in enumerate(zip(c_vals, r_vals)):
            if e != a:
                sys.stderr.write(f"Exact value mismatch at index {idx} in {context}: expected {e}, got {a}\n")
                return False
            if isinstance(e, float) and isinstance(a, float) and e == 0.0:
                if math.copysign(1.0, e) != math.copysign(1.0, a):
                    sys.stderr.write(f"Zero sign mismatch at index {idx} in {context}: expected {e}, got {a}\n")
                    return False
        return True
    elif kind == "abs_rel_tol":
        abs_tol = float(policy.get("abs", ABS_TOL))
        rel_tol = float(policy.get("rel", REL_TOL))
        for idx, (e, a) in enumerate(zip(c_vals, r_vals)):
            if e is None:
                if a is not None:
                    sys.stderr.write(f"NaN placement mismatch at index {idx} in {context}: expected NaN, got {a}\n")
                    return False
                continue
            if a is None:
                sys.stderr.write(f"NaN placement mismatch at index {idx} in {context}: expected {e}, got NaN\n")
                return False
            if isinstance(e, str) or isinstance(a, str):
                if e != a:
                    sys.stderr.write(f"String/Inf mismatch at index {idx} in {context}: expected {e}, got {a}\n")
                    return False
                continue
            e_f = float(e)
            a_f = float(a)
            diff = abs(a_f - e_f)
            if not (diff <= abs_tol or diff <= rel_tol * abs(e_f)):
                sys.stderr.write(
                    f"Tolerance mismatch at index {idx} in {context}: "
                    f"expected {e_f}, got {a_f} (diff {diff} > abs {abs_tol} and rel {rel_tol * abs(e_f)})\n"
                )
                return False
        return True
    else:
        sys.stderr.write(f"Unknown policy kind '{kind}' in {context}\n")
        return False


def compare_trees(committed: Path, regenerated: Path) -> int:
    """Compare two fixture trees. Returns 0 on success, 1 on difference."""
    committed_files = {p.relative_to(committed): p for p in committed.glob("**/*.json") if p.is_file()}
    regen_files = {p.relative_to(regenerated): p for p in regenerated.glob("**/*.json") if p.is_file()}

    if set(committed_files.keys()) != set(regen_files.keys()):
        missing = set(committed_files.keys()) - set(regen_files.keys())
        extra = set(regen_files.keys()) - set(committed_files.keys())
        sys.stderr.write(f"Tree structure mismatch. Missing: {missing}, Extra: {extra}\n")
        return 1

    failed = False
    for rel_path in sorted(committed_files.keys()):
        comm_file = committed_files[rel_path]
        regen_file = regen_files[rel_path]
        comm_bytes = comm_file.read_bytes()
        regen_bytes = regen_file.read_bytes()
        if comm_bytes == regen_bytes:
            continue

        comm_data = json.loads(comm_bytes)
        regen_data = json.loads(regen_bytes)

        comm_prov = comm_data.get("provenance", {})
        regen_prov = regen_data.get("provenance", {})
        comm_cpu = comm_prov.get("cpu_level")
        regen_cpu = regen_prov.get("cpu_level")

        if comm_prov.get("backend_rev") != regen_prov.get("backend_rev"):
            sys.stderr.write(
                f"backend_rev mismatch in {rel_path}: committed={comm_prov.get('backend_rev')} != regen={regen_prov.get('backend_rev')}\n"
            )
            failed = True
            continue

        if comm_cpu == regen_cpu:
            sys.stderr.write(f"Byte mismatch in {rel_path} with matching cpu_level {comm_cpu}\n")
            failed = True
            continue

        sys.stderr.write(f"Notice: cpu_level differs ({comm_cpu} vs {regen_cpu}) in {rel_path}\n")

        is_input = "inputs" in rel_path.parts or "columns" in comm_data
        if is_input:
            comm_cols = comm_data.get("columns", {})
            regen_cols = regen_data.get("columns", {})
            if set(comm_cols.keys()) != set(regen_cols.keys()):
                sys.stderr.write(f"Input columns mismatch in {rel_path}\n")
                failed = True
                continue
            for col_name in sorted(comm_cols.keys()):
                if not _compare_column_values(
                    comm_cols[col_name],
                    regen_cols[col_name],
                    {"kind": "exact"},
                    f"{rel_path}:{col_name}",
                ):
                    failed = True
            continue

        comm_cases = comm_data.get("cases", [])
        regen_cases = regen_data.get("cases", [])
        if len(comm_cases) != len(regen_cases):
            sys.stderr.write(f"Cases count mismatch in {rel_path}: {len(comm_cases)} vs {len(regen_cases)}\n")
            failed = True
            continue

        policy = comm_data.get("policy", {"kind": "exact"})
        for case_idx, (c_case, r_case) in enumerate(zip(comm_cases, regen_cases)):
            case_id = c_case.get("case_id")
            if case_id != r_case.get("case_id"):
                sys.stderr.write(f"Case ID mismatch at {case_idx} in {rel_path}: {case_id} vs {r_case.get('case_id')}\n")
                failed = True
                break
            if c_case.get("inputs") != r_case.get("inputs") or c_case.get("params") != r_case.get("params"):
                sys.stderr.write(f"Case metadata mismatch for {case_id} in {rel_path}\n")
                failed = True
                break
            c_exp = c_case.get("expected", {})
            r_exp = r_case.get("expected", {})
            if "rejected" in c_exp or "rejected" in r_exp:
                if c_exp != r_exp:
                    sys.stderr.write(f"Rejection mismatch for {case_id} in {rel_path}: {c_exp} vs {r_exp}\n")
                    failed = True
                continue
            c_outs = c_exp.get("outputs", {})
            r_outs = r_exp.get("outputs", {})
            if set(c_outs.keys()) != set(r_outs.keys()):
                sys.stderr.write(f"Outputs mismatch for {case_id} in {rel_path}: {set(c_outs.keys())} vs {set(r_outs.keys())}\n")
                failed = True
                continue
            for out_name in sorted(c_outs.keys()):
                if not _compare_column_values(
                    c_outs[out_name],
                    r_outs[out_name],
                    policy,
                    f"{rel_path}:{case_id}:{out_name}",
                ):
                    failed = True

    return 1 if failed else 0


class Family(Protocol):
    name: str  # subdirectory of the fixture root
    environment: Literal["numeric", "backend"]  # "backend" = q_backend's full locked env at BACKEND_REV
    policy: dict[str, object]  # envelope "policy"

    def export(self, source: BackendSource, out_dir: Path) -> list[str]:
        ...


def detect_environment(source: BackendSource | None = None) -> Literal["numeric", "backend"]:
    """Detect whether running in full backend environment or minimal numeric environment."""
    try:
        import q_backend  # noqa: F401

        return "backend"
    except ImportError:
        return "numeric"


from families.indicators import (  # noqa: E402
    FunctionSpec,
    IndicatorFamily,
    build_inputs,
    check_reference_causal,
    function_specs,
    run_case,
)

FAMILIES: Final[dict[str, Family]] = {
    "indicators": IndicatorFamily(),
}


def main(argv: list[str] | None = None) -> int:
    """CLI entry point for fixture exporter and tree comparator."""
    if argv is None:
        argv = sys.argv[1:]

    if argv and argv[0] == "compare":
        if len(argv) != 3:
            sys.stderr.write("Usage: export_reference.py compare COMMITTED REGENERATED\n")
            return 2
        return compare_trees(Path(argv[1]), Path(argv[2]))

    parser = argparse.ArgumentParser(description="Export reference fixtures from q_backend")
    parser.add_argument("--family", action="append", dest="families", help="Family to export (repeatable)")
    parser.add_argument(
        "--backend-repo",
        default="https://github.com/GuilhermeFortuna/q_backend.git",
        help="Repository URL for q_backend",
    )
    parser.add_argument("--rev-file", default="BACKEND_REV", help="File containing backend commit hash")
    parser.add_argument("--backend-checkout", help="Path to existing clean checkout of q_backend at rev")
    parser.add_argument(
        "--allow-numeric-in-backend",
        action="store_true",
        help="Allow running numeric families in backend environment",
    )
    parser.add_argument("--out", required=True, help="Output directory for reference fixtures")

    args = parser.parse_args(argv)
    out_dir = Path(args.out)

    detected_env = detect_environment()

    # Determine requested families
    if args.families:
        requested_families = args.families
    else:
        requested_families = [name for name, fam in FAMILIES.items() if fam.environment == detected_env]

    # Validate family environments
    for fam_name in requested_families:
        if fam_name not in FAMILIES:
            sys.stderr.write(f"Unknown family '{fam_name}'\n")
            return 2
        fam = FAMILIES[fam_name]
        if fam.environment == "backend" and detected_env == "numeric":
            sys.stderr.write(f"Family '{fam_name}' requires backend environment but running in numeric environment\n")
            return 2
        if fam.environment == "numeric" and detected_env == "backend" and not args.allow_numeric_in_backend:
            sys.stderr.write(
                f"Family '{fam_name}' is numeric-only; use --allow-numeric-in-backend to run in backend environment\n"
            )
            return 2

    # Get revision
    if args.rev_file:
        rev_path = Path(args.rev_file)
        if not rev_path.exists():
            sys.stderr.write(f"Revision file '{args.rev_file}' not found\n")
            return 2
        rev = rev_path.read_text(encoding="utf-8").strip()
    else:
        sys.stderr.write("Revision file not specified\n")
        return 2

    if args.backend_checkout:
        source = open_backend_checkout(Path(args.backend_checkout), rev)
        verify_environment(source)
        for fam_name in requested_families:
            FAMILIES[fam_name].export(source, out_dir)
    else:
        with tempfile.TemporaryDirectory() as tmpdir:
            source = fetch_backend(args.backend_repo, rev, Path(tmpdir))
            verify_environment(source)
            for fam_name in requested_families:
                FAMILIES[fam_name].export(source, out_dir)

    return 0


if __name__ == "__main__":
    sys.exit(main())
