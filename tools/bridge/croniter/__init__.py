"""Pure-Python shim satisfying `import croniter`.

Every public call here forwards to a long-lived `croniter-conformance` Rust
subprocess over the line-delimited JSON protocol described in
tools/bridge/run_original_suite.sh. This package exists only so the
ORIGINAL, UNMODIFIED croniter-python test suite (tests/original/) can run
unmodified against the Rust port (hackathon Rule 05 forbids the Rust crate
from linking/FFI-ing into Python, so the direction is inverted: Python
drives Rust as a subprocess instead of Rust embedding Python).

Stdlib only. No third-party dependencies.
"""

from __future__ import annotations

import atexit
import datetime
import json
import math
import os
import subprocess
import sys
import threading
from time import time
from typing import Any, Optional, Union

UTC_DT = datetime.timezone.utc
EPOCH = datetime.datetime.fromtimestamp(0, UTC_DT)

# ponytail: no 32-bit Python realistically runs this hackathon bridge.
OVERFLOW32B_MODE = False

MINUTE_FIELD = 0
HOUR_FIELD = 1
DAY_FIELD = 2
MONTH_FIELD = 3
DOW_FIELD = 4
SECOND_FIELD = 5
YEAR_FIELD = 6

UNIX_CRON_LEN = 5
SECOND_CRON_LEN = 6
YEAR_CRON_LEN = 7

# The set of field counts croniter accepts (croniter.py:137). Kept because the suite
# imports it directly to check its own expectations about expression lengths.
VALID_LEN_EXPRESSION = {UNIX_CRON_LEN, SECOND_CRON_LEN, YEAR_CRON_LEN}

__all__ = [
    "DAY_FIELD",
    "HOUR_FIELD",
    "MINUTE_FIELD",
    "MONTH_FIELD",
    "OVERFLOW32B_MODE",
    "SECOND_FIELD",
    "UTC_DT",
    "YEAR_FIELD",
    "CroniterBadCronError",
    "CroniterBadDateError",
    "CroniterBadTypeRangeError",
    "CroniterError",
    "CroniterNotAlphaError",
    "CroniterUnsupportedSyntaxError",
    "croniter",
    "croniter_range",
    "datetime_to_timestamp",
]


# ---------------------------------------------------------------------------
# Exceptions - original inheritance hierarchy, verbatim.
# ---------------------------------------------------------------------------


class CroniterError(ValueError):
    """General top-level Croniter base exception"""


class CroniterBadTypeRangeError(TypeError):
    """."""


class CroniterBadCronError(CroniterError):
    """Syntax, unknown value, or range error within a cron expression"""


class CroniterUnsupportedSyntaxError(CroniterBadCronError):
    """Valid cron syntax, but likely to produce inaccurate results"""


class CroniterBadDateError(CroniterError):
    """Unable to find next/prev timestamp match"""


class CroniterNotAlphaError(CroniterBadCronError):
    """Cron syntax contains an invalid day or month abbreviation"""


_ERROR_CLASSES = {
    "CroniterError": CroniterError,
    "CroniterBadCronError": CroniterBadCronError,
    "CroniterUnsupportedSyntaxError": CroniterUnsupportedSyntaxError,
    "CroniterBadDateError": CroniterBadDateError,
    "CroniterNotAlphaError": CroniterNotAlphaError,
    "CroniterBadTypeRangeError": CroniterBadTypeRangeError,
}


def datetime_to_timestamp(d: datetime.datetime) -> float:
    if d.tzinfo is not None:
        d = d.replace(tzinfo=None) - d.utcoffset()
    return (d - datetime.datetime(1970, 1, 1)).total_seconds()


_MARKER = object()


def _timestamp_to_datetime(timestamp: float, tzinfo: Optional[datetime.tzinfo]) -> datetime.datetime:
    result = EPOCH.replace(tzinfo=None) + datetime.timedelta(seconds=timestamp)
    if tzinfo:
        result = result.replace(tzinfo=UTC_DT).astimezone(tzinfo)
    return result


def _tz_name(tzinfo: Optional[datetime.tzinfo]) -> Optional[str]:
    """Best-effort IANA name for the wire protocol's `tz` field.

    `zoneinfo.ZoneInfo`/pytz objects carry a name; fixed-offset `timezone`
    objects don't. In the fixed-offset case we send `tz: null` and rely on
    the UTC offset embedded in the ISO8601 `start` string instead.
    """
    if tzinfo is None:
        return None
    key = getattr(tzinfo, "key", None)
    if isinstance(key, str):
        return key
    zone = getattr(tzinfo, "zone", None)
    if isinstance(zone, str):
        return zone
    # dateutil.tz.gettz() returns a tzfile, which carries neither `.key` nor `.zone` --
    # only the path it was loaded from. Falling through to None here would send a real
    # DST zone as a bare offset and lose every transition in it, which is precisely what
    # the DST tests are checking.
    filename = getattr(tzinfo, "_filename", None)
    if isinstance(filename, str) and filename:
        name = filename.split("zoneinfo/")[-1] if "zoneinfo/" in filename else filename
        if name and name != "localtime":
            return name
    return None


def _parse_iso(value: str, tzinfo: Optional[datetime.tzinfo]) -> datetime.datetime:
    dt = datetime.datetime.fromisoformat(value)
    if tzinfo is not None:
        if dt.tzinfo is not None:
            return dt.astimezone(tzinfo)
        return dt.replace(tzinfo=UTC_DT).astimezone(tzinfo)
    if dt.tzinfo is not None:
        offset = dt.utcoffset()
        return dt.replace(tzinfo=None) - offset
    return dt


def _is_second_precision(expr: str) -> bool:
    expr = expr.strip()
    if expr.startswith("@"):
        return False
    return len(expr.split()) > UNIX_CRON_LEN


# ---------------------------------------------------------------------------
# Rust subprocess transport
# ---------------------------------------------------------------------------


def _find_binary() -> str:
    override = os.environ.get("CRONITER_CONFORMANCE_BIN")
    if override:
        return override
    here = os.path.dirname(os.path.abspath(__file__))
    # tools/bridge/croniter/__init__.py -> tools/bridge -> tools -> repo root
    repo_root = os.path.dirname(os.path.dirname(os.path.dirname(here)))
    return os.path.join(repo_root, "target", "release", "croniter-conformance")


class _Bridge:
    """One persistent `croniter-conformance` subprocess for the whole session."""

    def __init__(self) -> None:
        self._proc: Optional[subprocess.Popen] = None
        self._lock = threading.Lock()
        self._next_id = 1

    def _ensure_started(self) -> None:
        if self._proc is not None and self._proc.poll() is None:
            return
        binary = _find_binary()
        if not os.path.isfile(binary) or not os.access(binary, os.X_OK):
            raise RuntimeError(
                f"croniter-conformance binary not found at {binary!r}. "
                "Build it first with `cargo build --release` (or `make build`), "
                "or point CRONITER_CONFORMANCE_BIN at an existing binary."
            )
        self._proc = subprocess.Popen(
            [binary],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=sys.stderr,
            text=True,
            bufsize=1,
        )

    def call(self, **request: Any) -> Any:
        with self._lock:
            self._ensure_started()
            assert self._proc is not None
            request_id = self._next_id
            self._next_id += 1
            request["id"] = request_id
            assert self._proc.stdin is not None and self._proc.stdout is not None
            self._proc.stdin.write(json.dumps(request) + "\n")
            self._proc.stdin.flush()
            reply_line = self._proc.stdout.readline()
            if not reply_line:
                rc = self._proc.poll()
                raise RuntimeError(
                    f"croniter-conformance exited unexpectedly (code={rc}) "
                    f"while handling {request!r}. Check stderr above."
                )
            reply = json.loads(reply_line)
        if reply.get("id") != request_id:
            raise RuntimeError(f"out-of-order response from croniter-conformance: {reply!r}")
        if not reply.get("ok"):
            error_name = reply.get("error", "CroniterError")
            cls = _ERROR_CLASSES.get(error_name, CroniterError)
            raise cls(reply.get("message", ""))
        return reply["value"]

    def shutdown(self) -> None:
        if self._proc is not None and self._proc.poll() is None:
            try:
                self._proc.stdin.close()  # type: ignore[union-attr]
            except Exception:
                pass
            try:
                self._proc.wait(timeout=2)
            except Exception:
                self._proc.kill()


_bridge = _Bridge()
atexit.register(_bridge.shutdown)


# ---------------------------------------------------------------------------
# croniter
# ---------------------------------------------------------------------------


class croniter:
    MONTHS_IN_YEAR = 12

    # Verbatim from croniter-python: (min, max) per field index. Pure data,
    # not engine logic, so it's safe to mirror directly rather than round
    # tripping through the Rust subprocess.
    RANGES = ((0, 59), (0, 23), (1, 31), (1, 12), (0, 6), (0, 59), (1970, 2099))

    def __init__(
        self,
        expr_format: str,
        start_time: Optional[Union[datetime.datetime, float]] = None,
        ret_type: type = float,
        day_or: bool = True,
        max_years_between_matches: Optional[int] = None,
        is_prev: bool = False,
        hash_id: Optional[Union[bytes, str]] = None,
        implement_cron_bug: bool = False,
        second_at_beginning: bool = False,
        expand_from_start_time: bool = False,
    ) -> None:
        self.expr_format = expr_format
        self._ret_type = ret_type
        self._day_or = day_or
        self._implement_cron_bug = implement_cron_bug
        self.second_at_beginning = bool(second_at_beginning)
        self._expand_from_start_time = expand_from_start_time

        if hash_id is not None:
            if not isinstance(hash_id, (bytes, str)):
                raise TypeError("hash_id must be bytes or UTF-8 string")
            if not isinstance(hash_id, bytes):
                hash_id = hash_id.encode("UTF-8")
        self._hash_id = hash_id

        self._max_years_btw_matches_explicitly_set = max_years_between_matches is not None
        if max_years_between_matches is None:
            max_years_between_matches = 50
        self._max_years_between_matches = max(int(max_years_between_matches), 1)

        if start_time is None:
            start_time = time()

        self.tzinfo: Optional[datetime.tzinfo] = None
        self.start_time = 0.0
        self.dst_start_time = 0.0
        self.cur = 0.0
        self.set_current(start_time, force=True)
        self._is_prev = is_prev
        self._expanded_cache: Optional[list] = None
        self._handle: Optional[int] = None

        # Parse once, on the engine side, and keep the handle. This mirrors the
        # original's eager `_expand()` in __init__ (a bad cron raises here), and it is
        # what makes `R` expressions behave: croniter draws their random values during
        # __init__ and reuses them for the object's lifetime, so the engine has to hold
        # that one parse rather than redo it per call.
        self._handle = self._request("create", ret="datetime")

    def __del__(self):
        # Best effort: let the engine drop the parse when this object goes away. A
        # failure here is never interesting -- interpreter teardown may already have
        # closed the pipe -- and must not surface from a destructor.
        handle = getattr(self, "_handle", None)
        if handle is None:
            return
        try:
            _bridge.call(op="destroy", expr=self.expr_format, handle=handle)
        except Exception:  # noqa: BLE001
            pass

    @property
    def expanded(self) -> list:
        """Lazily fetched, cached: the normalized per-field expansion of this
        instance's cron expression (list of lists of int | '*' | 'l'), via
        the `expand` wire op. Mirrors croniter-python's `.expanded` attribute."""
        if self._expanded_cache is None:
            # Goes through _request so the handle travels with it: this must report the
            # parse this instance is actually scheduling from, which for an `R`
            # expression is the draw made in __init__ and for expand_from_start_time is
            # the one anchored to this instance's start.
            value = self._request("expand", ret="datetime")
            self._expanded_cache = [list(field) for field in value["expanded"]]
        return self._expanded_cache

    # -- wire protocol helpers ------------------------------------------------

    def _args(self) -> dict:
        return {
            "day_or": self._day_or,
            # hash_id is arbitrary bytes -- croniter hashes it, it is never text -- and
            # the suite passes byte strings that are not valid UTF-8. Decoding it to put
            # it in JSON raises UnicodeDecodeError; hex survives the round trip exactly.
            "hash_id_hex": self._hash_id.hex() if self._hash_id else None,
            "implement_cron_bug": self._implement_cron_bug,
            "second_at_beginning": self.second_at_beginning,
            "expand_from_start_time": self._expand_from_start_time,
            "max_years_between_matches": self._max_years_between_matches,
        }

    def _request(self, op: str, *, ret: str) -> Any:
        start_dt = _timestamp_to_datetime(self.cur, self.tzinfo)
        return _bridge.call(
            op=op,
            expr=self.expr_format,
            start=start_dt.isoformat(),
            tz=_tz_name(self.tzinfo),
            ret=ret,
            args=self._args(),
            # None on the `create` call that mints it; set for everything after.
            handle=getattr(self, "_handle", None),
        )

    # -- public API, mirroring croniter-python ---------------------------------

    def get_next(self, ret_type=None, start_time=None, update_current=True):
        if start_time and self._expand_from_start_time:
            raise ValueError(
                "start_time is not supported when using expand_from_start_time = True."
            )
        return self._get_next(
            ret_type=ret_type, start_time=start_time, is_prev=False, update_current=update_current
        )

    def get_prev(self, ret_type=None, start_time=None, update_current=True):
        return self._get_next(
            ret_type=ret_type, start_time=start_time, is_prev=True, update_current=update_current
        )

    def _get_next(self, ret_type=None, start_time=None, is_prev=None, update_current=None):
        if update_current is None:
            update_current = True
        self.set_current(start_time, force=True)
        if is_prev is None:
            is_prev = self._is_prev
        self._is_prev = is_prev

        ret_type = ret_type or self._ret_type
        if not issubclass(ret_type, (float, datetime.datetime)):
            raise TypeError("Invalid ret_type, only 'float' or 'datetime' is acceptable.")

        value = self._request("prev" if is_prev else "next", ret="datetime")
        result = _parse_iso(value, self.tzinfo)
        timestamp = datetime_to_timestamp(result)
        if update_current:
            self.cur = timestamp
        if issubclass(ret_type, datetime.datetime):
            return result
        return timestamp

    def get_current(self, ret_type=None):
        ret_type = ret_type or self._ret_type
        if issubclass(ret_type, datetime.datetime):
            return self.timestamp_to_datetime(self.cur)
        return self.cur

    def set_current(self, start_time, force: bool = True) -> float:
        if (force or (self.cur is None)) and start_time is not None:
            if isinstance(start_time, datetime.datetime):
                self.tzinfo = start_time.tzinfo
                start_time = self.datetime_to_timestamp(start_time)
            self.start_time = start_time
            self.dst_start_time = start_time
            self.cur = start_time
        return self.cur

    @staticmethod
    def datetime_to_timestamp(d: datetime.datetime) -> float:
        return datetime_to_timestamp(d)

    _datetime_to_timestamp = datetime_to_timestamp  # retrocompat

    def timestamp_to_datetime(self, timestamp: float, tzinfo: Any = _MARKER) -> datetime.datetime:
        if tzinfo is _MARKER:
            tzinfo = self.tzinfo
        return _timestamp_to_datetime(timestamp, tzinfo)

    _timestamp_to_datetime = timestamp_to_datetime  # retrocompat

    # -- iterator protocol ------------------------------------------------------

    def all_next(self, ret_type=None, start_time=None, update_current=None):
        try:
            while True:
                self._is_prev = False
                yield self._get_next(
                    ret_type=ret_type, start_time=start_time, is_prev=False, update_current=update_current
                )
                start_time = None
        except CroniterBadDateError:
            if self._max_years_btw_matches_explicitly_set:
                return
            raise

    def all_prev(self, ret_type=None, start_time=None, update_current=None):
        try:
            while True:
                self._is_prev = True
                yield self._get_next(
                    ret_type=ret_type, start_time=start_time, is_prev=True, update_current=update_current
                )
                start_time = None
        except CroniterBadDateError:
            if self._max_years_btw_matches_explicitly_set:
                return
            raise

    def iter(self, *args, **kwargs):
        return self.all_prev if self._is_prev else self.all_next

    def __iter__(self):
        return self

    def __next__(self, ret_type=None, start_time=None, is_prev=None, update_current=None):
        return self._get_next(
            ret_type=ret_type, start_time=start_time, is_prev=is_prev, update_current=update_current
        )

    next = __next__

    @staticmethod
    def _get_nth_weekday_of_month(year: int, month: int, day_of_week: int):
        """Calendar utility (no cron-engine logic): days-of-month in nth-weekday
        order, mirrored verbatim from croniter-python since it's pure stdlib
        `calendar` math, not something the Rust engine computes or exposes."""
        import calendar

        w = (day_of_week + 6) % 7
        c = calendar.Calendar(w).monthdayscalendar(year, month)
        if c[0][0] == 0:
            c.pop(0)
        return tuple(i[0] for i in c)

    @classmethod
    def expand(
        cls,
        expr_format,
        hash_id=None,
        second_at_beginning=False,
        from_timestamp=None,
        from_timestamp_tz=None,
        strict=False,
        strict_year=None,
        **_ignored,
    ):
        """Returns `(expanded, nth_weekday_of_month)`, exactly as
        croniter-python's `expand()` does (see its docstring for shape)."""
        if hash_id is not None:
            if not isinstance(hash_id, (bytes, str)):
                raise TypeError("hash_id must be bytes or UTF-8 string")
            if not isinstance(hash_id, bytes):
                hash_id = hash_id.encode("UTF-8")
        args = {"second_at_beginning": bool(second_at_beginning)}
        if hash_id is not None:
            args["hash_id_hex"] = hash_id.hex()
        if from_timestamp is not None:
            args["from_timestamp"] = float(from_timestamp)
            tz_name = _tz_name(from_timestamp_tz)
            if tz_name is not None:
                args["from_timestamp_tz"] = tz_name
        if strict:
            args["strict"] = True
            args["strict_year"] = (
                []
                if strict_year is None
                else [int(strict_year)]
                if isinstance(strict_year, int)
                else [int(y) for y in strict_year]
            )
        value = _bridge.call(op="expand", expr=expr_format, args=args)
        expanded = [list(field) for field in value["expanded"]]
        nth_weekday_of_month = {int(k): set(v) for k, v in value["nth_weekday_of_month"].items()}
        return expanded, nth_weekday_of_month

    @classmethod
    def is_valid(
        cls,
        expression,
        hash_id=None,
        encoding="UTF-8",
        second_at_beginning=False,
        strict=False,
        strict_year=None,
    ) -> bool:
        if hash_id:
            if not isinstance(hash_id, (bytes, str)):
                raise TypeError("hash_id must be bytes or UTF-8 string")
            if not isinstance(hash_id, bytes):
                hash_id = hash_id.encode(encoding)
        try:
            _bridge.call(
                op="validate",
                expr=expression,
                start=datetime.datetime.now(UTC_DT).isoformat(),
                tz=None,
                ret="datetime",
                args={
                    "hash_id_hex": hash_id.hex() if hash_id else None,
                    "second_at_beginning": bool(second_at_beginning),
                    "strict": bool(strict),
                    # Upstream accepts a bare int or a list; the wire carries a list.
                    "strict_year": (
                        []
                        if strict_year is None
                        else [int(strict_year)]
                        if isinstance(strict_year, int)
                        else [int(y) for y in strict_year]
                    ),
                },
            )
        except CroniterError:
            return False
        return True

    @classmethod
    def match(cls, cron_expression, testdate, day_or=True, second_at_beginning=False, precision_in_seconds=None):
        return cls.match_range(
            cron_expression, testdate, testdate, day_or, second_at_beginning, precision_in_seconds
        )

    @classmethod
    def match_range(
        cls,
        cron_expression,
        from_datetime,
        to_datetime,
        day_or=True,
        second_at_beginning=False,
        precision_in_seconds=None,
    ):
        cron = cls(
            cron_expression,
            to_datetime,
            ret_type=datetime.datetime,
            day_or=day_or,
            second_at_beginning=second_at_beginning,
        )
        tdp = cron.get_current(datetime.datetime)
        if not tdp.microsecond:
            tdp += datetime.timedelta(microseconds=1)
        cron.set_current(tdp, force=True)
        try:
            tdt = cron.get_prev()
        except CroniterBadDateError:
            return False
        if precision_in_seconds is None:
            precision_in_seconds = 1 if _is_second_precision(cron_expression) else 60
        duration_in_second = (to_datetime - from_datetime).total_seconds() + precision_in_seconds
        return (max(tdp, tdt) - min(tdp, tdt)).total_seconds() < duration_in_second


def croniter_range(
    start,
    stop,
    expr_format,
    ret_type=None,
    day_or=True,
    exclude_ends=False,
    _croniter=None,
    second_at_beginning=False,
    expand_from_start_time=False,
):
    """Generator of all times from `start` to `stop` matching `expr_format`.

    Mirrors croniter-python's implementation, which is itself just a
    get_next/get_prev loop, so this forwards to the same Rust subprocess via
    the `croniter` class above with no new wire-protocol calls needed.
    """
    _croniter = _croniter or croniter
    auto_rt = datetime.datetime
    if type(start) is not type(stop) and not (
        isinstance(start, type(stop)) or isinstance(stop, type(start))
    ):
        raise CroniterBadTypeRangeError(
            f"The start and stop must be same type.  {type(start)} != {type(stop)}"
        )
    if isinstance(start, (float, int)):
        start, stop = (
            datetime.datetime.fromtimestamp(t, UTC_DT).replace(tzinfo=None) for t in (start, stop)
        )
        auto_rt = float
    if ret_type is None:
        ret_type = auto_rt
    if not exclude_ends:
        ms1 = datetime.timedelta(microseconds=1)
        if start < stop:
            start -= ms1
            stop += ms1
        else:
            start += ms1
            stop -= ms1
    year_span = math.floor(abs(stop.year - start.year)) + 1
    ic = _croniter(
        expr_format,
        start,
        ret_type=datetime.datetime,
        day_or=day_or,
        max_years_between_matches=year_span,
        second_at_beginning=second_at_beginning,
        expand_from_start_time=expand_from_start_time,
    )
    if start < stop:

        def cont(v):
            return v < stop

        step = ic.get_next
    else:

        def cont(v):
            return v > stop

        step = ic.get_prev

    try:
        dt = step()
        while cont(dt):
            if ret_type is float:
                yield ic.get_current(float)
            else:
                yield dt
            dt = step()
    except CroniterBadDateError:
        return


# Re-exported for parity with croniter-python's `__init__.py`, which exposes
# the module itself as `cron_m` (some tests import `croniter.cron_m`).
cron_m = sys.modules[__name__]

# Upstream the implementation lives in the submodule `croniter.croniter` and the package
# re-exports from it, so the suite is free to import from either. This shim is flat, so
# alias the submodule name onto the package rather than splitting the file in two just to
# reproduce a directory layout. `from croniter.croniter import VALID_LEN_EXPRESSION` then
# resolves the same way it does upstream.
sys.modules[f"{__name__}.croniter"] = cron_m
