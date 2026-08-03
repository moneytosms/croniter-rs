"""Maps `croniter.tests` onto the verbatim upstream suite in `tests/original/`.

Upstream, the suite lives inside the package at `croniter/tests/`, so its modules
import each other as `from croniter.tests import base`. `tests/original/` is that
directory copied byte-for-byte, which means those imports have to keep resolving
without editing a single test file. Pointing this package's `__path__` at it does
that: `croniter.tests.base` loads `tests/original/base.py`.

Stdlib only, like the rest of the shim.
"""

from __future__ import annotations

import os

_HERE = os.path.dirname(os.path.abspath(__file__))
# tools/bridge/croniter/tests -> repo root
_REPO_ROOT = os.path.abspath(os.path.join(_HERE, "..", "..", "..", ".."))
_ORIGINAL = os.path.join(_REPO_ROOT, "tests", "original")

if not os.path.isdir(_ORIGINAL):  # pragma: no cover - misconfigured checkout
    raise ImportError(
        f"expected the upstream suite at {_ORIGINAL}, but that directory does not exist"
    )

__path__ = [_ORIGINAL]
