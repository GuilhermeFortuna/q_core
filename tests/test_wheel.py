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

    with tempfile.TemporaryDirectory() as tmpdir:
        venv_dir = Path(tmpdir) / "venv"
        subprocess.check_call(["uv", "venv", str(venv_dir)], stdout=subprocess.DEVNULL)
        python_bin = venv_dir / "bin" / "python"
        subprocess.check_call(
            ["uv", "pip", "install", "--python", str(python_bin), str(wheel_path)],
            stdout=subprocess.DEVNULL,
        )

        code = f"""
import q_core
v = q_core.version()
c = q_core.contracts_rev()
print(f"wheel version: {{v}}")
print(f"wheel contracts_rev: {{c}}")
assert v == {expected_version!r}, f"Version mismatch: {{v}} != {expected_version!r}"
assert c == {expected_rev!r}, f"Contracts rev mismatch: {{c}} != {expected_rev!r}"
"""
        res = subprocess.run([str(python_bin), "-c", code], capture_output=True, text=True)
        if res.returncode != 0:
            sys.stderr.write(res.stderr)
            sys.exit(res.returncode)
        print(res.stdout.strip())
        print("Wheel integration test passed successfully.")


if __name__ == "__main__":
    main()
