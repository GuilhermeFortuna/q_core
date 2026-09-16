# Releasing `q_core`

## Release Identification

Releases of `q_core` are identified by date tags, **not** semantic versions:

```
vYYYY.MM.DD
vYYYY.MM.DD.n   # if multiple releases occur on the same day
```

Semantic versioning makes compatibility assertions that cannot be validated without running the full parity suite. In this architecture, compatibility across repository boundaries is governed strictly by the cross-repository compatibility record in `q_contracts/COMPAT.md` and verified by golden parity runs.

The workspace Cargo/`pyproject.toml` version uses the unpadded form `YYYY.M.D` (for example `2026.9.14`); the git tag uses zero-padded `vYYYY.MM.DD` (for example `v2026.09.14`).

**The package version does not identify a release; the tag does.** `YYYY.M.D` has
no room for the same-day `.n` suffix — `2026.9.15` is a valid Cargo and PEP 440
version, `2026.9.15.2` is neither — so `v2026.09.15` and `v2026.09.15.2` both
report `q_core.version() == "2026.9.15"`. A consumer that needs to know which
release it has must read the resolved commit its lockfile records, not the
version string. `uv.lock` stores it on the `source` line of the `q-core`
package; `cargo` stores it in `Cargo.lock`.

---

## Release Procedure

Releases are cut by the workspace launcher, not by hand. Finishing a reviewed task in this
repository does the whole procedure:

```bash
./work finish Q-029
```

It bumps `[workspace.package] version` in `Cargo.toml` and `version` in `pyproject.toml` to
today's date in `YYYY.M.D` form, refreshes `Cargo.lock`, runs `make check`, tags
`vYYYY.MM.DD` (suffixed `.2`, `.3`, … for a second release the same day), and pushes
`development` and the tag. If `make check` fails, nothing is tagged or pushed and the task
stays `In Review`; fix the cause on `development` and run the same command again.

Because the tag is what consumers pin, a task that is `Done` here is always one `q_backend`
or `q_terminal` can consume. See `q/docs/work-cli.md`.

### By hand

Only needed when releasing without a board task — for example a documentation-only release:

1. **Pre-flight Validation**: ensure the working tree is clean (`git status`).
2. **Bump package versions**: set `[workspace.package] version` in `Cargo.toml` and `version`
   in `pyproject.toml` to today's date in `YYYY.M.D` form (unpadded, matching existing values
   such as `2026.9.12`), and refresh `Cargo.lock`.
3. **Execute Validation Suite**: `make check` must pass with zero warnings or errors.
4. **Create Git Tag**: `git tag v2026.09.14`.
5. **Publish Tag**: `git push origin development && git push origin v2026.09.14`.

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

`version()` is the `YYYY.M.D` package version, which two same-day releases
share. To confirm *which* release is installed, read the commit `uv` resolved
the tag to:
```bash
grep -A2 '^name = "q-core"' uv.lock | grep source
```

### 2. Qt Host (`q_terminal`)
The terminal host references `q_core` via its CMake / Cargo build configuration pinned to the release tag.

---

## Cross-Repository Compatibility Record

Whenever a release is cut and adopted by a consumer:
1. Update `q_contracts/COMPAT.md` with the new pin and consumer commit.
2. Record the verification evidence.
