# Releasing `q_core`

## Release Identification

Releases of `q_core` are identified by date tags, **not** semantic versions:

```
vYYYY.MM.DD
vYYYY.MM.DD.n   # if multiple releases occur on the same day
```

Semantic versioning makes compatibility assertions that cannot be validated without running the full parity suite. In this architecture, compatibility across repository boundaries is governed strictly by the cross-repository compatibility record in `q_contracts/COMPAT.md` and verified by golden parity runs.

The workspace Cargo/`pyproject.toml` version uses the unpadded form `YYYY.M.D` (for example `2026.9.14`); the git tag uses zero-padded `vYYYY.MM.DD` (for example `v2026.09.14`).

---

## Release Procedure

1. **Pre-flight Validation**:
   Ensure all local changes are committed and the working tree is clean:
   ```bash
   git status
   ```

2. **Bump package versions**:
   Set `[workspace.package] version` in `Cargo.toml` and `version` in `pyproject.toml` to
   today's release date in `YYYY.M.D` form (month and day without zero-padding, matching
   existing values such as `2026.9.12`). Refresh `Cargo.lock` (for example via
   `cargo metadata` or a workspace build) so workspace crate versions match. Confirm
   `q_core.version()` will report the new value after the next wheel build.

3. **Execute Validation Suite**:
   Run the canonical validation suite:
   ```bash
   make check
   ```
   All checks (rustfmt, clippy, cargo tests, wheel build, clean venv wheel installation test, Qt C++ harness test, and contracts regeneration check) must pass with zero warnings or errors.

4. **Create Git Tag**:
   Tag the commit using the release date format:
   ```bash
   git tag v2026.09.14
   ```

5. **Publish Tag**:
   Push the tag to the remote repository:
   ```bash
   git push origin v2026.09.14
   ```

---

## Consumer Pinning

### 1. Python Host (`q_backend`)
The backend pins to the release tag via `uv`:

In `q_backend/pyproject.toml`:
```toml
[project]
dependencies = [
    # ...
    "q-core",
]

[tool.uv.sources]
q-core = { git = "https://github.com/GuilhermeFortuna/q_core.git", tag = "v2026.09.14" }
```

Run resolution and installation:
```bash
uv sync
```

Verify the installed module:
```bash
uv run python -c "import q_core; print(q_core.version(), q_core.contracts_rev())"
```

### 2. Qt Host (`q_terminal`)
The terminal host references `q_core` via its CMake / Cargo build configuration pinned to the release tag.

---

## Cross-Repository Compatibility Record

Whenever a release is cut and adopted by a consumer:
1. Update `q_contracts/COMPAT.md` with the new pin and consumer commit.
2. Record the verification evidence.
