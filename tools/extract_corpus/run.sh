#!/usr/bin/env bash
# Recreates the venv and regenerates tests/port/corpus.json from scratch.
#
# The corpus is the only place Python touches this project, and it is a
# build-time data source: the committed tests/port/corpus.json is what the Rust
# test suite replays. Nothing here runs at test time.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$HERE/../.." && pwd)"
CORPUS_OUT="$REPO_ROOT/tests/port/corpus.json"

# The commit the port is pinned to. Must match .port-mortem.toml's source_commit;
# extracting against anything else would silently rebase the oracle.
SOURCE_COMMIT="f64665eb635402af70b4225832b37e53d0b35727"
SOURCE_URL="https://github.com/pallets-eco/croniter"

# Prefer a sibling checkout if the developer has one, otherwise fetch the pinned
# commit into a gitignored scratch dir. Either way the commit is verified before
# extraction, so the corpus can never be built against a different source.
CRONITER_PYTHON="${CRONITER_PYTHON:-$REPO_ROOT/../croniter-python}"
if [ ! -d "$CRONITER_PYTHON" ]; then
    CRONITER_PYTHON="$HERE/.source/croniter"
    if [ ! -d "$CRONITER_PYTHON/.git" ]; then
        echo "==> No croniter-python checkout found; fetching $SOURCE_COMMIT"
        rm -rf "$HERE/.source"
        mkdir -p "$CRONITER_PYTHON"
        git init -q "$CRONITER_PYTHON"
        git -C "$CRONITER_PYTHON" remote add origin "$SOURCE_URL"
        git -C "$CRONITER_PYTHON" fetch -q --depth 1 origin "$SOURCE_COMMIT"
        git -C "$CRONITER_PYTHON" checkout -q FETCH_HEAD
    fi
fi

ACTUAL_COMMIT="$(git -C "$CRONITER_PYTHON" rev-parse HEAD)"
if [ "$ACTUAL_COMMIT" != "$SOURCE_COMMIT" ]; then
    echo "ERROR: $CRONITER_PYTHON is at $ACTUAL_COMMIT, expected $SOURCE_COMMIT." >&2
    echo "       The corpus must be extracted from the pinned source commit." >&2
    exit 1
fi
echo "==> Extracting against $CRONITER_PYTHON @ $SOURCE_COMMIT"

rm -rf "$HERE/.venv"
uv venv "$HERE/.venv" --python 3.12
uv pip install --python "$HERE/.venv/bin/python" \
    -e "$CRONITER_PYTHON" "pytest>=8.3.3" "pytz>2021.1"

# PYTHONPATH so `-p corpus_plugin` resolves regardless of cwd.
CORPUS_OUT_PATH="$CORPUS_OUT" PYTHONPATH="$HERE${PYTHONPATH:+:$PYTHONPATH}" \
    "$HERE/.venv/bin/python" -m pytest \
    -p corpus_plugin -q "$CRONITER_PYTHON/src/croniter/tests"

echo "corpus written to $CORPUS_OUT"
