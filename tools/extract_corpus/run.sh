#!/usr/bin/env bash
# Recreates the venv and regenerates tests/port/corpus.json from scratch.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CRONITER_PYTHON="$HERE/../../../croniter-python"
CORPUS_OUT="$HERE/../../tests/port/corpus.json"

rm -rf "$HERE/.venv"
uv venv "$HERE/.venv" --python 3.12
uv pip install --python "$HERE/.venv/bin/python" \
    -e "$CRONITER_PYTHON" "pytest>=8.3.3" "pytz>2021.1"

CORPUS_OUT_PATH="$CORPUS_OUT" "$HERE/.venv/bin/python" -m pytest \
    -p corpus_plugin -q "$CRONITER_PYTHON/src/croniter/tests"

echo "corpus written to $CORPUS_OUT"
