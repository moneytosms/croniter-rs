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

if ! command -v pytest >/dev/null 2>&1; then
    echo "ERROR: pytest not found on PATH. Install it (e.g. \`pip install pytest\`) and retry." >&2
    exit 1
fi

# Bridge shim ahead of everything else, so `import croniter` resolves to our
# pure-Python subprocess shim rather than any real croniter package.
export PYTHONPATH="$BRIDGE_DIR${PYTHONPATH:+:$PYTHONPATH}"
export CRONITER_CONFORMANCE_BIN="$BINARY"

echo "==> Running original croniter-python test suite against the Rust port"
set +e
pytest "$REPO_ROOT/tests/original" "$@"
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
