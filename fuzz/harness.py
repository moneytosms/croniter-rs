#!/usr/bin/env python3
"""Differential fuzz harness: real Python croniter (oracle) vs the Rust port
(target/release/croniter-conformance, one long-lived subprocess).

Usage:
    fuzz/harness.py --seconds 60 --seed 1 --log fuzz/log.txt

Requires the venv at tools/extract_corpus/.venv (has croniter + dateutil
installed editable). Run with that venv's python, or this script re-execs
into it automatically.
"""
from __future__ import annotations

import argparse
import datetime as dt
import hashlib
import json
import os
import random
import string
import subprocess
import sys
import time
from pathlib import Path
from zoneinfo import ZoneInfo

REPO = Path(__file__).resolve().parent.parent
VENV_PY = REPO / "tools" / "extract_corpus" / ".venv" / "bin" / "python"
BINARY = REPO / "target" / "release" / "croniter-conformance"

# Re-exec under the venv python so `import croniter` resolves, unless we're
# already running under it.
if sys.executable != str(VENV_PY) and VENV_PY.exists():
    os.execv(str(VENV_PY), [str(VENV_PY), __file__, *sys.argv[1:]])

from croniter import croniter as PyCroniter  # noqa: E402
from croniter.croniter import (  # noqa: E402
    CroniterBadCronError,
    CroniterBadDateError,
    CroniterBadTypeRangeError,
    CroniterError,
    CroniterNotAlphaError,
    CroniterUnsupportedSyntaxError,
)

try:
    from croniter.croniter import croniter_range as py_croniter_range  # noqa: E402
except ImportError:
    from croniter import croniter_range as py_croniter_range  # noqa: E402

OPS = ["next", "prev", "current", "validate", "expand", "range", "match", "match_range"]

# Highest year the two timezone databases still agree on.
#
# chrono-tz's generated transition table for a zone runs out after its last explicit
# transition and then holds the final offset forever; Python's zoneinfo keeps applying the
# POSIX TZ footer rule indefinitely. For Australia/Sydney they agree through 2099 and part
# ways in 2100, where chrono-tz reports +11:00 (AEDT) in June -- Australian winter -- while
# zoneinfo correctly reports +10:00. Generating starts past this point measures the two tz
# databases against each other rather than the two croniter implementations, which is not
# what this harness is for. See DECISIONS.md section 19.
MAX_TZ_AGREED_YEAR = 2099

# A case slower than this is reported separately rather than silently dragging the rate.
SLOW_CASE_SECONDS = 5.0

DST_TZS = ["America/New_York", "Europe/London", "Australia/Sydney"]

# A handful of real DST transition instants (UTC-naive wall clock, local to
# the zone) for the zones above. Picked by hand from well known 2024/2025
# transitions; harness also mutates these with +/- a few minutes.
DST_INSTANTS = [
    ("America/New_York", "2024-03-10T02:00:00"),  # spring forward (gap)
    ("America/New_York", "2024-11-03T01:30:00"),  # fall back (ambiguous)
    ("Europe/London", "2024-03-31T01:00:00"),
    ("Europe/London", "2024-10-27T01:30:00"),
    ("Australia/Sydney", "2024-04-07T02:30:00"),
    ("Australia/Sydney", "2024-10-06T02:30:00"),
]

MONTH_NAMES = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"]
DOW_NAMES = ["sun", "mon", "tue", "wed", "thu", "fri", "sat"]
NICKNAMES = ["@yearly", "@annually", "@monthly", "@weekly", "@daily", "@midnight", "@hourly"]


# ---------------------------------------------------------------------------
# Cron expression generation
# ---------------------------------------------------------------------------

def _int_field(rng: random.Random, lo: int, hi: int) -> str:
    return str(rng.randint(lo, hi))


def _list_field(rng: random.Random, lo: int, hi: int, names: list[str] | None = None) -> str:
    n = rng.randint(2, 4)
    vals = []
    for _ in range(n):
        if names and rng.random() < 0.3:
            vals.append(rng.choice(names))
        else:
            vals.append(str(rng.randint(lo, hi)))
    return ",".join(vals)


def _range_field(rng: random.Random, lo: int, hi: int) -> str:
    a = rng.randint(lo, hi - 1)
    b = rng.randint(a, hi)
    return f"{a}-{b}"


def _step_field(rng: random.Random, lo: int, hi: int) -> str:
    step = rng.randint(1, max(2, (hi - lo) // 2 or 1))
    if rng.random() < 0.5:
        return f"*/{step}"
    a = rng.randint(lo, hi - 1)
    b = rng.randint(a, hi)
    return f"{a}-{b}/{step}"


def gen_field(rng: random.Random, kind: str) -> str:
    """kind in {second, minute, hour, day, month, dow, year}."""
    ranges = {
        "second": (0, 59, None),
        "minute": (0, 59, None),
        "hour": (0, 23, None),
        "day": (1, 31, None),
        "month": (1, 12, MONTH_NAMES),
        "dow": (0, 6, DOW_NAMES),
        "year": (1970, MAX_TZ_AGREED_YEAR, None),
    }
    lo, hi, names = ranges[kind]
    choice = rng.random()
    if choice < 0.25:
        return "*"
    if choice < 0.40:
        return _int_field(rng, lo, hi) if not names or rng.random() < 0.5 else rng.choice(names)
    if choice < 0.55:
        return _list_field(rng, lo, hi, names)
    if choice < 0.70:
        return _range_field(rng, lo, hi)
    if choice < 0.85:
        return _step_field(rng, lo, hi)
    # special tokens
    if kind == "day":
        specials = ["L", f"{rng.randint(1,28)}W", "LW", "?"]
        return rng.choice(specials)
    if kind == "dow":
        specials = [f"{rng.randint(0,6)}#{rng.randint(1,5)}", "?", "L"]
        return rng.choice(specials)
    return _int_field(rng, lo, hi)


def gen_valid_cron(rng: random.Random) -> str:
    if rng.random() < 0.08:
        return rng.choice(NICKNAMES)

    form = rng.choices([5, 6, 7], weights=[0.55, 0.30, 0.15])[0]
    minute = gen_field(rng, "minute")
    hour = gen_field(rng, "hour")
    day = gen_field(rng, "day")
    month = gen_field(rng, "month")
    dow = gen_field(rng, "dow")
    fields = [minute, hour, day, month, dow]

    if form == 6:
        # 6-field: seconds prepended by convention in this codebase's grammar
        second = gen_field(rng, "second")
        fields = [second] + fields
    elif form == 7:
        second = gen_field(rng, "second")
        year = gen_field(rng, "year")
        fields = [second] + fields + [year]

    return " ".join(fields)


def gen_malformed_cron(rng: random.Random) -> str:
    junk = [
        "",
        "* * *",
        "* * * * * * * *",
        "60 * * * *",
        "* 24 * * *",
        "* * 32 * *",
        "* * * 13 *",
        "* * * * 8",
        "abc * * * *",
        "* * * foo *",
        "*/0 * * * *",
        "1-60 * * * *",
        "5-1 * * * *",
        "@every5min",
        "* * * * * */",
        ",,,* * * * *",
        "* * * * *,",
        "-- * * * *",
        "* * * * * L L",
        "9#6 * * * *",
        "0 0 30 2 *",  # syntactically fine, semantically impossible date
    ]
    return rng.choice(junk)


def gen_cron(rng: random.Random) -> str:
    if rng.random() < 0.15:
        return gen_malformed_cron(rng)
    return gen_valid_cron(rng)


# ---------------------------------------------------------------------------
# Start datetime generation
# ---------------------------------------------------------------------------

def gen_naive_dt(rng: random.Random) -> dt.datetime:
    choice = rng.random()
    if choice < 0.15:
        # month boundary
        month = rng.randint(1, 12)
        year = rng.randint(1990, 2035)
        day = 1 if rng.random() < 0.5 else 28
        base = dt.datetime(year, month, day)
        return base
    if choice < 0.30:
        # Feb 29 in a leap year, or attempted in a non-leap year (caller must
        # catch ValueError for the latter -> use a leap year to stay valid)
        leap_years = [2020, 2024, 2028, 2000]
        year = rng.choice(leap_years)
        return dt.datetime(year, 2, 29, rng.randint(0, 23), rng.randint(0, 59), rng.randint(0, 59))
    if choice < 0.45:
        # 31st of a 30-day month rolled back to the 30th to stay constructible;
        # the "edge" is exercised by cron fields expecting day 31 that never occurs.
        month = rng.choice([4, 6, 9, 11])
        year = rng.randint(1990, 2035)
        return dt.datetime(year, month, 30, rng.randint(0, 23), rng.randint(0, 59))
    if choice < 0.60:
        # year boundary
        year = rng.randint(1990, 2035)
        if rng.random() < 0.5:
            return dt.datetime(year, 12, 31, 23, 59, rng.randint(0, 59))
        return dt.datetime(year, 1, 1, 0, 0, rng.randint(0, 59))
    if choice < 0.75:
        # DST transition instant, taken naive (tz applied separately)
        _, iso = rng.choice(DST_INSTANTS)
        base = dt.datetime.fromisoformat(iso)
        return base + dt.timedelta(minutes=rng.randint(-5, 5))
    # otherwise uniform-ish random date
    year = rng.randint(1970, MAX_TZ_AGREED_YEAR)
    month = rng.randint(1, 12)
    day = rng.randint(1, 28)
    return dt.datetime(year, month, day, rng.randint(0, 23), rng.randint(0, 59), rng.randint(0, 59))


def gen_start(rng: random.Random) -> tuple[dt.datetime, str | None]:
    """Returns (naive datetime, tz name or None)."""
    naive = gen_naive_dt(rng)
    if rng.random() < 0.5:
        return naive, None
    tz = rng.choice(DST_TZS + ["UTC", "Asia/Tokyo"])
    return naive, tz


# ---------------------------------------------------------------------------
# Port (Rust) subprocess wrapper
# ---------------------------------------------------------------------------

class Port:
    def __init__(self, binary: Path):
        self.proc = subprocess.Popen(
            [str(binary)],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            bufsize=1,
        )
        self._id = 0

    def call(self, op: str, expr: str, start: str | None, tz: str | None,
              ret: str | None, n: int | None, stop: str | None, args: dict) -> dict:
        self._id += 1
        req = {
            "id": self._id, "op": op, "expr": expr, "start": start, "tz": tz,
            "ret": ret, "n": n, "stop": stop, "args": args,
        }
        line = json.dumps(req)
        assert self.proc.stdin is not None and self.proc.stdout is not None
        self.proc.stdin.write(line + "\n")
        self.proc.stdin.flush()
        out = self.proc.stdout.readline()
        if not out:
            err = self.proc.stderr.read() if self.proc.stderr else ""
            raise RuntimeError(f"port process died: {err}")
        return json.loads(out)

    def close(self):
        try:
            self.proc.stdin.close()
        except Exception:
            pass
        self.proc.terminate()


# ---------------------------------------------------------------------------
# Oracle (Python) side
# ---------------------------------------------------------------------------

def localize(naive: dt.datetime, tz: str | None) -> dt.datetime | float:
    if tz is None:
        return naive
    return naive.replace(tzinfo=ZoneInfo(tz))  # fold=0 == "earliest", matches Rust's .earliest()


def py_error_class(exc: Exception) -> str:
    # Most specific subclasses first (CroniterNotAlphaError and
    # CroniterUnsupportedSyntaxError both extend CroniterBadCronError).
    for cls in (CroniterNotAlphaError, CroniterUnsupportedSyntaxError, CroniterBadDateError,
                CroniterBadTypeRangeError, CroniterBadCronError, CroniterError):
        if isinstance(exc, cls):
            return cls.__name__
    return type(exc).__name__


def run_oracle(op: str, expr: str, naive: dt.datetime, tz: str | None,
               ret: str, n: int, stop_naive: dt.datetime | None, args: dict):
    """Returns ("ok", value) or ("err", class_name, message)."""
    try:
        if op == "validate":
            ok = PyCroniter.is_valid(expr, hash_id=args.get("hash_id"),
                                      second_at_beginning=args.get("second_at_beginning", False))
            return ("ok", ok)

        if op == "expand":
            expanded, nth = PyCroniter.expand(
                expr, hash_id=args.get("hash_id"),
                second_at_beginning=args.get("second_at_beginning", False),
            )
            norm = [list(field) for field in expanded]
            norm_nth = {str(k): sorted(v) for k, v in nth.items()}
            return ("ok", {"expanded": norm, "nth_weekday_of_month": norm_nth})

        if op in ("match", "match_range"):
            # bridge parses start/stop as naive always (tz field unused for these ops)
            if op == "match":
                res = PyCroniter.match(expr, naive, day_or=args.get("day_or", True))
            else:
                res = PyCroniter.match_range(expr, naive, stop_naive, day_or=args.get("day_or", True))
            return ("ok", res)

        if op == "range":
            items = list(py_croniter_range(
                naive, stop_naive, expr,
                ret_type=dt.datetime,
                day_or=args.get("day_or", True),
                exclude_ends=args.get("exclude_ends", False),
                # Must be forwarded: with second_at_beginning the leading field is
                # seconds, so the two sides otherwise parse different expressions and
                # every generated 6-field case looks like a divergence.
                second_at_beginning=args.get("second_at_beginning", False),
            ))
            return ("ok", [d.strftime("%Y-%m-%dT%H:%M:%S.%f") for d in items])

        # next / prev / current
        start_val = localize(naive, tz)
        cron = PyCroniter(
            expr, start_val, ret_type=dt.datetime,
            day_or=args.get("day_or", True),
            max_years_between_matches=args.get("max_years_between_matches"),
            hash_id=args.get("hash_id"),
            implement_cron_bug=args.get("implement_cron_bug", False),
            second_at_beginning=args.get("second_at_beginning", False),
        )
        rt = dt.datetime if ret == "datetime" else float
        if op == "next":
            v = cron.get_next(rt)
        elif op == "prev":
            v = cron.get_prev(rt)
        else:
            v = cron.get_current(rt)
        if isinstance(v, dt.datetime):
            v = v.strftime("%Y-%m-%dT%H:%M:%S.%f%z")
        return ("ok", v)
    except Exception as exc:  # noqa: BLE001 - comparing exception classes deliberately
        return ("err", py_error_class(exc), str(exc))


# ---------------------------------------------------------------------------
# Normalization / comparison
# ---------------------------------------------------------------------------

def norm_datetime_str(s):
    """Strip trailing zero fractional seconds so '.000000' == no fraction, and
    drop UTC offset formatting quirks (+00:00 vs Z is not produced by either
    side here, but normalise anyway)."""
    if not isinstance(s, str):
        return s
    s2 = s
    # Split off the timezone offset if present, and normalise its shape. The oracle
    # formats with %z ("+0000") while the port emits "+00:00"; those denote the same
    # offset, and comparing them raw flags every timezone-aware case as a divergence,
    # which buries the real ones.
    tz_suffix = ""
    if len(s2) >= 6 and s2[-6] in "+-" and s2[-3] == ":":
        tz_suffix = s2[-6:]
        s2 = s2[:-6]
    elif len(s2) >= 5 and s2[-5] in "+-" and s2[-4:].isdigit():
        tz_suffix = f"{s2[-5:-2]}:{s2[-2:]}"
        s2 = s2[:-5]
    elif s2.endswith("Z"):
        tz_suffix = "+00:00"
        s2 = s2[:-1]
    if tz_suffix in ("+00:00", "-00:00"):
        tz_suffix = "+00:00"
    if "." in s2:
        head, frac = s2.split(".", 1)
        frac = frac.rstrip("0")
        s2 = head if frac == "" else f"{head}.{frac}"
    return s2 + tz_suffix


def values_match(a, b) -> bool:
    if isinstance(a, str) and isinstance(b, str):
        return norm_datetime_str(a) == norm_datetime_str(b)
    if isinstance(a, (int, float)) and isinstance(b, (int, float)) and not isinstance(a, bool) and not isinstance(b, bool):
        return abs(float(a) - float(b)) < 1e-6
    if isinstance(a, list) and isinstance(b, list):
        return len(a) == len(b) and all(values_match(x, y) for x, y in zip(a, b))
    if isinstance(a, dict) and isinstance(b, dict):
        # Port's "expand" response carries extra fields (expressions,
        # nearest_weekday) the oracle side doesn't reconstruct; only compare
        # keys present on both sides.
        common = a.keys() & b.keys()
        if not common:
            return a == b
        return all(values_match(a[k], b[k]) for k in common)
    return a == b


# ---------------------------------------------------------------------------
# Case generation + comparison driver
# ---------------------------------------------------------------------------

def gen_case(rng: random.Random) -> dict:
    op = rng.choice(OPS)
    expr = gen_cron(rng)
    naive, tz = gen_start(rng)
    n = rng.randint(1, 5)
    ret = rng.choice(["datetime", "float"])
    args = {
        "day_or": rng.random() < 0.85,
        "second_at_beginning": rng.random() < 0.2,
        "implement_cron_bug": rng.random() < 0.1,
        "max_years_between_matches": rng.choice([None, 1, 5, 50]),
        # Bytes, not str. croniter's `expand()` classmethod feeds hash_id straight to
        # binascii.crc32 without encoding it, so a str raises TypeError there while the
        # constructor accepts one -- an asymmetry in the oracle, not a port behaviour.
        # The port's hash_id is `Option<&[u8]>` and cannot be a str at all, so generating
        # bytes is what actually compares the two implementations.
        "hash_id": rng.choice([None, b"abc", b"worker-1"]) if rng.random() < 0.15 else None,
        "exclude_ends": rng.random() < 0.3,
    }
    stop_naive = None
    if op in ("range", "match_range"):
        delta = dt.timedelta(days=rng.choice([1, 7, 30, 365]))
        stop_naive = naive + delta if rng.random() < 0.5 else naive - delta
        if stop_naive < naive:
            naive, stop_naive = stop_naive, naive
    return {
        "op": op, "expr": expr, "naive": naive, "tz": tz, "n": n, "ret": ret,
        "args": args, "stop_naive": stop_naive,
    }


def case_to_wire(case: dict) -> dict:
    # The wire carries hash_id hex-encoded, because croniter hashes arbitrary bytes and
    # not every byte string is valid UTF-8. Sending the old plain `hash_id` field means
    # the port silently sees no hash_id at all, which turns e.g. `@yearly` into literal
    # "0 0 1 1 *" on one side and a hashed schedule on the other.
    args = dict(case["args"])
    hash_id = args.pop("hash_id", None)
    if hash_id is not None:
        raw = hash_id if isinstance(hash_id, bytes) else str(hash_id).encode("UTF-8")
        args["hash_id_hex"] = raw.hex()
    return {
        "op": case["op"],
        "expr": case["expr"],
        "start": case["naive"].isoformat(),
        "tz": case["tz"],
        "ret": case["ret"],
        "n": case["n"],
        "stop": case["stop_naive"].isoformat() if case["stop_naive"] else None,
        "args": args,
    }


def run_one(port: Port, case: dict):
    wire = case_to_wire(case)
    port_resp = port.call(wire["op"], wire["expr"], wire["start"], wire["tz"],
                           wire["ret"], wire["n"], wire["stop"], wire["args"])
    oracle_result = run_oracle(case["op"], case["expr"], case["naive"], case["tz"],
                                case["ret"], case["n"], case["stop_naive"], case["args"])

    if port_resp.get("ok"):
        port_status = ("ok", port_resp.get("value"))
    else:
        port_status = ("err", port_resp.get("error"), port_resp.get("message"))

    # The `validate` wire op reports an invalid expression by *raising*, because that is
    # what its two real callers need: the bridge's `__init__` has to reproduce croniter's
    # eager `_expand()`, and the bridge's `is_valid` catches the raise and returns False.
    # `croniter.is_valid()` on the oracle side answers with a bool instead. A raise here
    # and a `False` there are the same verdict, so normalise before comparing rather than
    # reporting every rejected expression as a divergence.
    if case["op"] == "validate" and oracle_result == ("ok", False) and port_status[0] == "err":
        port_status = ("ok", False)

    verdict = "match"
    detail = ""
    if oracle_result[0] == "ok" and port_status[0] == "ok":
        if not values_match(oracle_result[1], port_status[1]):
            verdict = "divergence"
            detail = f"value mismatch: oracle={oracle_result[1]!r} port={port_status[1]!r}"
    elif oracle_result[0] == "err" and port_status[0] == "err":
        if oracle_result[1] != port_status[1]:
            verdict = "divergence"
            detail = f"error class mismatch: oracle={oracle_result[1]} port={port_status[1]}"
        elif oracle_result[2] != port_status[2]:
            verdict = "warning"
            detail = f"message mismatch: oracle={oracle_result[2]!r} port={port_status[2]!r}"
    else:
        verdict = "divergence"
        detail = f"oracle={oracle_result} port={port_status}"

    return wire, oracle_result, port_status, verdict, detail


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

def versions() -> tuple[str, str]:
    try:
        commit = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=REPO, text=True).strip()
    except Exception:
        commit = "unknown"
    py_ver = f"python-croniter (path={PyCroniter.__module__})"
    return commit, py_ver


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--seconds", type=float, default=60)
    ap.add_argument("--seed", type=int, default=1)
    ap.add_argument("--log", type=Path, default=REPO / "fuzz" / "log.txt")
    args = ap.parse_args()

    rng = random.Random(args.seed)
    if not BINARY.exists():
        print(f"missing binary: {BINARY}. Build with `cargo build --release` first.", file=sys.stderr)
        sys.exit(1)

    port = Port(BINARY)
    commit, py_ver = versions()

    log_lines: list[str] = []
    header = (
        f"# seed={args.seed} duration_s={args.seconds} port_commit={commit} "
        f"oracle={py_ver} port_binary={BINARY}\n"
    )

    total = 0
    divergences: list[str] = []
    slow_cases: list[tuple[float, str]] = []
    warnings = 0
    detail_cap = 200
    start_t = time.monotonic()

    try:
        while time.monotonic() - start_t < args.seconds:
            case = gen_case(rng)
            case_t0 = time.monotonic()
            try:
                wire, oracle_result, port_status, verdict, detail = run_one(port, case)
            except Exception as exc:  # noqa: BLE001 - keep the loop alive, log the crash
                verdict = "harness_error"
                detail = f"{type(exc).__name__}: {exc}"
                wire = case_to_wire(case)
                oracle_result = None
                port_status = None
            case_secs = time.monotonic() - case_t0

            # A generated `croniter_range` can span decades at one-second granularity,
            # which is tens of millions of results on both sides. That is a property of
            # the generator, not a divergence -- but it dominates the run's throughput,
            # so record it instead of leaving an unexplained stall in the rate.
            if case_secs >= SLOW_CASE_SECONDS:
                slow_cases.append((round(case_secs, 1), json.dumps(case_to_wire(case), sort_keys=True)))

            total += 1
            case_repr = json.dumps(wire, sort_keys=True)
            case_hash = hashlib.sha1(case_repr.encode()).hexdigest()[:12]

            if verdict == "warning":
                warnings += 1

            if verdict in ("divergence", "harness_error"):
                repro = {
                    "wire": wire,
                    "oracle": oracle_result,
                    "port": port_status,
                    "detail": detail,
                }
                line = f"DIVERGENCE #{len(divergences)+1} hash={case_hash} {json.dumps(repro, default=str)}"
                divergences.append(line)
                log_lines.append(line)
                print(f"\n!! {verdict}: {detail}")
            elif total <= detail_cap:
                log_lines.append(f"case hash={case_hash} verdict={verdict} {case_repr}")
            else:
                log_lines.append(f"case hash={case_hash} verdict={verdict}")

            if total % 200 == 0:
                elapsed = time.monotonic() - start_t
                print(f"\rcases={total} elapsed={elapsed:.1f}s rate={total/elapsed:.1f}/s "
                      f"divergences={len(divergences)} warnings={warnings}", end="", flush=True)
    finally:
        port.close()

    elapsed = time.monotonic() - start_t
    rate = total / elapsed if elapsed else 0.0
    summary = (
        f"\n# SUMMARY cases={total} elapsed_s={elapsed:.2f} rate_per_s={rate:.1f} "
        f"divergences={len(divergences)} warnings={warnings} slow_cases={len(slow_cases)}\n"
    )
    if slow_cases:
        summary += (
            f"# {len(slow_cases)} case(s) took >={SLOW_CASE_SECONDS}s and dominate the rate above.\n"
            "# These are generator artefacts (a range spanning years at second granularity\n"
            "# is tens of millions of results on both sides), not divergences.\n"
        )
        for secs, repr_ in sorted(slow_cases, reverse=True)[:5]:
            summary += f"#   {secs}s {repr_}\n"
    print(summary)

    args.log.parent.mkdir(parents=True, exist_ok=True)
    with open(args.log, "w") as f:
        f.write(header)
        f.write("\n".join(log_lines))
        f.write("\n")
        f.write(summary)

    print(f"log written to {args.log}")


if __name__ == "__main__":
    main()
