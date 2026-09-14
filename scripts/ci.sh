#!/usr/bin/env bash
set -euo pipefail

# Enter the host user ci.slice when available so local CI yields to interactive work.
# No-ops on hosts/runners without systemd-run or the slice (e.g. GitHub Actions).
if [[ "${CI_RESOURCE_CONTROLLED:-0}" != "1" ]]; then
  if command -v systemd-run >/dev/null 2>&1 &&
     systemctl --user status ci.slice >/dev/null 2>&1; then
    exec systemd-run \
      --user --scope --quiet --collect \
      --slice=ci.slice \
      --setenv=CI_RESOURCE_CONTROLLED=1 \
      "$0" "$@"
  fi
fi

# Local CI for q_core: runs the same Makefile targets as `make check` (what GitHub CI
# runs), cheapest first, with a prerequisite preflight and resource limits so a cold
# build does not saturate the desktop. The Makefile stays the source of truth.

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

echo "=========================================="
echo " Starting q_core CI Pipeline"
echo "=========================================="

# 1. Prerequisites
# qt-build-utils downloads a minimal Qt whose tools (qtpaths) link these system
# libraries. Without them every Cargo build of q-qt — clippy included — panics with a
# loader error deep in a build script, so check up front and say what to install.
echo "==> [1/7] Checking prerequisites..."
MISSING=()
for tool in cargo python3 g++ make git; do
  command -v "$tool" >/dev/null 2>&1 || MISSING+=("$tool")
done
if ! command -v maturin >/dev/null 2>&1 && ! command -v uvx >/dev/null 2>&1; then
  MISSING+=("maturin (or uv, for uvx maturin)")
fi
if command -v ldconfig >/dev/null 2>&1; then
  # Read the cache once: `ldconfig -p | grep -q` trips pipefail when grep exits early.
  LD_CACHE="$(ldconfig -p)"
  for lib in libdouble-conversion.so.3 libpcre2-16.so.0 libglib-2.0.so.0; do
    [[ "$LD_CACHE" == *"$lib "* ]] || MISSING+=("$lib")
  done
fi
if [ "${#MISSING[@]}" -gt 0 ]; then
  echo "Missing prerequisites: ${MISSING[*]}"
  echo "On Debian/Ubuntu: sudo apt-get install build-essential libdouble-conversion3 libpcre2-16-0 libglib2.0-0t64"
  echo "See README.md (Build and runtime prerequisites)."
  exit 1
fi

# Locally, cap Cargo build parallelism at half the logical CPUs and lower CPU/IO
# priority so cold builds yield to interactive work. Override with CARGO_BUILD_JOBS=N.
# On CI runners (CI set) Cargo keeps its defaults.
NICE=()
if [ -z "${CI:-}" ]; then
  CPUS="$(nproc 2>/dev/null || echo 2)"
  DEFAULT_JOBS=$(( CPUS / 2 ))
  (( DEFAULT_JOBS < 1 )) && DEFAULT_JOBS=1
  export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-$DEFAULT_JOBS}"
  echo "Cargo build jobs: ${CARGO_BUILD_JOBS}"
  if command -v nice >/dev/null 2>&1; then
    NICE=(nice -n 10)
    command -v ionice >/dev/null 2>&1 && NICE=(ionice -c 3 "${NICE[@]}")
  fi
fi

run_stage() {
  local label="$1" target="$2"
  echo "==> ${label} (make ${target})..."
  "${NICE[@]}" make --no-print-directory "$target"
}

run_stage "[2/7] Checking formatting" fmt-check
# Needs to reach the contracts repository; point CONTRACTS_REPO at a local clone when offline.
run_stage "[3/7] Checking vendored contracts" contracts-check
run_stage "[4/7] Running clippy" lint
run_stage "[5/7] Running workspace tests" test
run_stage "[6/7] Building q-qt and running the C++ harness" qt-test
run_stage "[7/7] Building release wheel and running wheel test" wheel-test

echo "=========================================="
echo " All q_core CI checks passed successfully!"
echo "=========================================="
