"""
pytest plugin: wraps the public croniter API and records every call the
original test suite makes, along with its actual return value or raised
exception. Dumped as a flat JSON array at session end.

Never imported by / injected into croniter-python itself: loaded purely as
an external pytest plugin (`-p corpus_plugin`) so the reference repo stays
untouched.
"""
import datetime
import json
import os

RECORDS = []
OUT_PATH = os.environ["CORPUS_OUT_PATH"]


def tz_name(tzinfo):
    if tzinfo is None:
        return None
    if hasattr(tzinfo, "zone"):  # pytz
        return tzinfo.zone
    if hasattr(tzinfo, "key"):  # zoneinfo.ZoneInfo
        return tzinfo.key
    if tzinfo == datetime.timezone.utc:
        return "UTC"
    return str(tzinfo)


def jsonify(v):
    if isinstance(v, datetime.datetime):
        return v.isoformat()
    if isinstance(v, (int, float)):
        return float(v)
    return str(v)


def ret_kind(v):
    return "datetime" if isinstance(v, datetime.datetime) else "float"


def start_fields(start_time):
    """-> (start_iso_or_None, tz_or_None)"""
    if isinstance(start_time, datetime.datetime):
        return start_time.isoformat(), tz_name(start_time.tzinfo)
    if isinstance(start_time, (int, float)):
        return float(start_time), None
    return None, None


def record(op, expr, start, tz, ret, args, ok, value=None, exc=None):
    if ok:
        expect = {"ok": True, "value": value}
    else:
        expect = {"ok": False, "error": type(exc).__name__, "message": str(exc)}
    RECORDS.append(
        {
            "op": op,
            "expr": expr,
            "start": start,
            "tz": tz,
            "ret": ret,
            "args": args,
            "expect": expect,
        }
    )


def pytest_configure(config):
    import croniter as pkg_early

    cron_m = pkg_early.cron_m  # the submodule; `croniter.croniter` attr is shadowed by the class

    Croniter = cron_m.croniter
    orig_init = Croniter.__init__
    orig_get_next = Croniter.get_next
    orig_get_prev = Croniter.get_prev
    orig_get_current = Croniter.get_current
    orig_all_next = Croniter.all_next
    orig_all_prev = Croniter.all_prev
    orig_set_current = Croniter.set_current
    orig_range = cron_m.croniter_range

    depth = [0]  # >0 while inside another wrapped public call -> nested/internal, don't record

    CTOR_EXTRA_KEYS = (
        "day_or",
        "max_years_between_matches",
        "is_prev",
        "hash_id",
        "implement_cron_bug",
        "second_at_beginning",
        "expand_from_start_time",
    )

    def wrapped_init(self, expr_format, start_time=None, ret_type=float, **kwargs):
        s_start, s_tz = start_fields(start_time)
        args = {k: v for k, v in kwargs.items() if k in CTOR_EXTRA_KEYS}
        # hash_id may be bytes; keep JSON-safe
        if "hash_id" in args and isinstance(args["hash_id"], bytes):
            args["hash_id"] = args["hash_id"].decode("utf-8", "replace")
        ret_str = "datetime" if isinstance(ret_type, type) and issubclass(ret_type, datetime.datetime) else "float"
        depth[0] += 1  # __init__'s internal set_current() call isn't a real "current" op
        try:
            orig_init(self, expr_format, start_time=start_time, ret_type=ret_type, **kwargs)
        except Exception as exc:  # noqa: BLE001 - deliberately broad, recording actual behavior
            record("validate", expr_format, s_start, s_tz, ret_str, args, False, exc=exc)
            raise
        finally:
            depth[0] -= 1
        record("validate", expr_format, s_start, s_tz, ret_str, args, True, value=True)

    def _call_and_record(op, self, orig_fn, ret_type_kw, start_time_kw, extra_args, *a, **kw):
        expr = self._expr_format if hasattr(self, "_expr_format") else None
        s_start, s_tz = start_fields(start_time_kw)
        effective_ret_type = ret_type_kw or getattr(self, "_ret_type", float)
        ret_str = (
            "datetime"
            if isinstance(effective_ret_type, type) and issubclass(effective_ret_type, datetime.datetime)
            else "float"
        )
        try:
            result = orig_fn(self, *a, **kw)
        except Exception as exc:  # noqa: BLE001
            record(op, expr, s_start, s_tz, ret_str, extra_args, False, exc=exc)
            raise
        record(op, expr, s_start, s_tz, ret_kind(result), extra_args, True, value=jsonify(result))
        return result

    def wrapped_get_next(self, ret_type=None, start_time=None, update_current=True):
        return _call_and_record(
            "next", self, orig_get_next, ret_type, start_time, {},
            ret_type=ret_type, start_time=start_time, update_current=update_current,
        )

    def wrapped_get_prev(self, ret_type=None, start_time=None, update_current=True):
        return _call_and_record(
            "prev", self, orig_get_prev, ret_type, start_time, {},
            ret_type=ret_type, start_time=start_time, update_current=update_current,
        )

    def wrapped_get_current(self, ret_type=None):
        expr = getattr(self, "_expr_format", None)
        try:
            result = orig_get_current(self, ret_type=ret_type)
        except Exception as exc:  # noqa: BLE001
            record("current", expr, None, None, "float", {}, False, exc=exc)
            raise
        record("current", expr, None, None, ret_kind(result), {}, True, value=jsonify(result))
        return result

    def wrapped_set_current(self, start_time, force=True):
        if getattr(self, "_suppress_current", False):
            return orig_set_current(self, start_time, force=force)
        expr = getattr(self, "_expr_format", None)
        s_start, s_tz = start_fields(start_time)
        try:
            result = orig_set_current(self, start_time, force=force)
        except Exception as exc:  # noqa: BLE001
            record("current", expr, s_start, s_tz, "float", {"force": force}, False, exc=exc)
            raise
        record(
            "current", expr, s_start, s_tz, "float", {"force": force}, True, value=jsonify(result)
        )
        return result

    def _wrap_all(op, orig_fn):
        def wrapped(self, ret_type=None, start_time=None, update_current=None):
            s_start, s_tz = start_fields(start_time)
            gen = orig_fn(self, ret_type=ret_type, start_time=start_time, update_current=update_current)
            values = []
            rec = {
                "op": op,
                "expr": None,
                "start": s_start,
                "tz": s_tz,
                "ret": "float",
                "args": {},
                "expect": {"ok": True, "value": values},
            }
            RECORDS.append(rec)
            try:
                for item in gen:
                    values.append(jsonify(item))
                    rec["ret"] = ret_kind(item)
                    yield item
            except Exception as exc:  # noqa: BLE001
                rec["expect"] = {"ok": False, "error": type(exc).__name__, "message": str(exc)}
                raise

        return wrapped

    def wrapped_range(start, stop, expr_format, ret_type=None, **kwargs):
        s_start, s_tz = start_fields(start)
        s_stop, _ = start_fields(stop)
        args = {k: v for k, v in kwargs.items() if k != "_croniter"}
        args["stop"] = s_stop
        gen = orig_range(start, stop, expr_format, ret_type=ret_type, **kwargs)
        values = []
        rec = {
            "op": "range",
            "expr": expr_format,
            "start": s_start,
            "tz": s_tz,
            "ret": "float",
            "args": args,
            "expect": {"ok": True, "value": values},
        }
        RECORDS.append(rec)
        try:
            for item in gen:
                values.append(jsonify(item))
                rec["ret"] = ret_kind(item)
                yield item
        except Exception as exc:  # noqa: BLE001
            rec["expect"] = {"ok": False, "error": type(exc).__name__, "message": str(exc)}
            raise

    # store expr_format on instances for later ops to report it
    def init_and_stash(self, expr_format, *a, **kw):
        try:
            wrapped_init(self, expr_format, *a, **kw)
        finally:
            self._expr_format = expr_format

    Croniter.__init__ = init_and_stash
    Croniter.get_next = wrapped_get_next
    Croniter.get_prev = wrapped_get_prev
    Croniter.get_current = wrapped_get_current
    Croniter.set_current = wrapped_set_current
    Croniter.all_next = _wrap_all("all_next", orig_all_next)
    Croniter.all_prev = _wrap_all("all_prev", orig_all_prev)
    cron_m.croniter_range = wrapped_range

    # also patch the re-export in the package __init__ so `from croniter import croniter_range` works
    import croniter as pkg

    pkg.croniter_range = wrapped_range


def _dedup(records):
    seen = set()
    out = []
    for r in records:
        key = json.dumps(r, sort_keys=True, default=str)
        if key in seen:
            continue
        seen.add(key)
        out.append(r)
    return out


def pytest_sessionfinish(session, exitstatus):
    deduped = _dedup(RECORDS)
    os.makedirs(os.path.dirname(OUT_PATH), exist_ok=True)
    with open(OUT_PATH, "w") as f:
        json.dump(deduped, f, indent=2, default=str)
    print(f"\ncorpus_plugin: wrote {len(deduped)} records ({len(RECORDS)} before dedup) to {OUT_PATH}")
