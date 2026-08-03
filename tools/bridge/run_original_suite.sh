#!/usr/bin/env bash
# Runs the ORIGINAL, UNMODIFIED croniter-python test suite (tests/original/)
# against the Rust port, via the pure-Python shim in tools/bridge/croniter.
#
# Usage: tools/bridge/run_original_suite.sh [pytest args...]
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
BRIDGE_DIR="$SCRIPT_DIR"
BINARY="$REPO_ROOT/target/release/croniter-conformance"

echo "==> Building croniter-conformance (cargo build --release)"
( cd "$REPO_ROOT" && cargo build --release --bin croniter-conformance )

if [ ! -x "$BINARY" ]; then
    echo "ERROR: expected Rust binary at $BINARY but it is missing or not executable." >&2
    echo "       cargo build --release did not produce it - check the build output above." >&2
    exit 1
fi

echo "==> Using croniter-conformance at $BINARY"

# A dedicated venv holding pytest and nothing else. Deliberately NOT the corpus
# extractor's venv: that one has the real croniter installed, and the entire point here
# is that `import croniter` resolves to the shim sitting in front of the Rust binary.
#
# The suite's own imports (dateutil, pytz) are test dependencies of croniter, not of
# croniter itself, so they belong here rather than being anything the port needs.
BRIDGE_DEPS=("pytest>=8.3.3" "python-dateutil>=2.9.0" "pytz>2021.1")
VENV="$SCRIPT_DIR/.venv"
if [ ! -x "$VENV/bin/pytest" ]; then
    echo "==> Creating bridge venv at $VENV (pytest only)"
    if command -v uv >/dev/null 2>&1; then
        uv venv "$VENV" --python 3.12
        uv pip install --python "$VENV/bin/python" "${BRIDGE_DEPS[@]}"
    elif command -v python3 >/dev/null 2>&1; then
        python3 -m venv "$VENV"
        "$VENV/bin/pip" install --quiet "${BRIDGE_DEPS[@]}"
    else
        echo "ERROR: need uv or python3 on PATH to create the bridge venv." >&2
        exit 1
    fi
fi

# Bridge shim ahead of everything else, so `import croniter` resolves to our
# pure-Python subprocess shim rather than any real croniter package.
export PYTHONPATH="$BRIDGE_DIR${PYTHONPATH:+:$PYTHONPATH}"
export CRONITER_CONFORMANCE_BIN="$BINARY"

echo "==> Running original croniter-python test suite against the Rust port"
set +e
"$VENV/bin/pytest" "$REPO_ROOT/tests/original" "$@"
STATUS=$?
set -e

echo "----------------------------------------------------------------------"
if [ "$STATUS" -eq 0 ]; then
    echo "PASS: original croniter-python suite passes against the Rust port."
else
    echo "FAIL: original croniter-python suite reported failures (pytest exit $STATUS)."
fi
echo "----------------------------------------------------------------------"

exit "$STATUS"
