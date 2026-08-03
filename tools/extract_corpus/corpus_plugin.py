"""
pytest plugin: wraps the public croniter API and records every call the
original test suite makes, along with its actual return value or raised
exception. Dumped as a flat JSON array at session end.

Never imported by / injected into croniter-python itself: loaded purely as
an external pytest plugin (`-p corpus_plugin`) so the reference repo stays
untouched.

Every record is meant to be independently replayable: construct a croniter
at `expr`/`start`/`tz`, invoke `op` once, compare against `expect`. That
means `expr` and `start` are never null - `start` is the instance's actual
cursor position at the moment of the call (captured/derived before the
wrapped method runs), not just whatever start_time argument (if any) the
test happened to pass in that call.
"""
import datetime
import json
import os

RECORDS = []
OUT_PATH = os.environ["CORPUS_OUT_PATH"]


def describe_tz(tzinfo):
    """-> dict with 'tz' (IANA name or None) and, for unnamed/fixed-offset
    tzinfo, 'tz_kind'/'tz_offset_seconds' so a harness can skip rather than
    silently mis-replay."""
    if tzinfo is None:
        return {"tz": None}
    if hasattr(tzinfo, "zone"):  # pytz
        return {"tz": tzinfo.zone}
    if hasattr(tzinfo, "key"):  # zoneinfo.ZoneInfo
        return {"tz": tzinfo.key}
    if tzinfo == datetime.timezone.utc:
        return {"tz": "UTC"}
    try:
        offset = tzinfo.utcoffset(None)
        offset_seconds = int(offset.total_seconds()) if offset is not None else None
    except Exception:  # noqa: BLE001
        offset_seconds = None
    return {"tz": None, "tz_kind": "fixed_offset", "tz_offset_seconds": offset_seconds}


def jsonify(v):
    if isinstance(v, datetime.datetime):
        return v.isoformat()
    if isinstance(v, (int, float)):
        return float(v)
    return str(v)


def ret_kind(v):
    return "datetime" if isinstance(v, datetime.datetime) else "float"


def iso_or_none(start_time):
    """Best-effort ISO8601 for a raw constructor start_time argument (may be
    a datetime, a float timestamp, or None)."""
    if isinstance(start_time, datetime.datetime):
        return start_time.isoformat()
    if isinstance(start_time, (int, float)):
        return datetime.datetime.fromtimestamp(float(start_time), datetime.timezone.utc).isoformat()
    return None


def record(op, expr, start, tz_fields, ret, args, ok, value=None, exc=None):
    if ok:
        expect = {"ok": True, "value": value}
    else:
        expect = {"ok": False, "error": type(exc).__name__, "message": str(exc)}
    rec = {
        "op": op,
        "expr": expr,
        "start": start,
        "ret": ret,
        "args": args,
        "expect": expect,
    }
    rec.update(tz_fields)
    RECORDS.append(rec)
    return rec


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
    orig_timestamp_to_datetime = Croniter.timestamp_to_datetime
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

    def effective_position(self, start_time_param):
        """(iso_start, tz_fields) for the position a call is actually anchored
        at: the explicit start_time argument if one was passed, else the
        instance's current cursor (self.cur) from *before* the call runs."""
        if isinstance(start_time_param, datetime.datetime):
            timestamp = Croniter.datetime_to_timestamp(start_time_param)
            tzinfo = start_time_param.tzinfo
        elif isinstance(start_time_param, (int, float)):
            timestamp = float(start_time_param)
            tzinfo = getattr(self, "tzinfo", None)
        else:
            timestamp = getattr(self, "cur", None)
            tzinfo = getattr(self, "tzinfo", None)
            if timestamp is None:
                # instance blew up before self.cur was ever assigned (e.g. a
                # bad hash_id type raised before set_current ran) - fall back
                # to wall clock so `start` is still never null.
                return datetime.datetime.now().isoformat(), describe_tz(tzinfo)
        dt = orig_timestamp_to_datetime(self, timestamp, tzinfo=tzinfo)
        return dt.isoformat(), describe_tz(tzinfo)

    def ctor_start_of(self):
        return getattr(self, "_corpus_ctor_start", None)

    def wrapped_init(self, expr_format, start_time=None, ret_type=float, **kwargs):
        ctor_start_iso = iso_or_none(start_time)
        args = {k: v for k, v in kwargs.items() if k in CTOR_EXTRA_KEYS}
        # hash_id may be bytes; keep JSON-safe
        if "hash_id" in args and isinstance(args["hash_id"], bytes):
            args["hash_id"] = args["hash_id"].decode("utf-8", "replace")
        args["ctor_start"] = ctor_start_iso
        ret_str = "datetime" if isinstance(ret_type, type) and issubclass(ret_type, datetime.datetime) else "float"
        depth[0] += 1  # __init__'s internal set_current() call isn't a real "current" op
        try:
            orig_init(self, expr_format, start_time=start_time, ret_type=ret_type, **kwargs)
        except Exception as exc:  # noqa: BLE001 - deliberately broad, recording actual behavior
            s_start, tzf = effective_position(self, None)
            record("validate", expr_format, s_start, tzf, ret_str, args, False, exc=exc)
            raise
        finally:
            depth[0] -= 1
        s_start, tzf = effective_position(self, None)
        record("validate", expr_format, s_start, tzf, ret_str, args, True, value=True)

    def _call_and_record(op, self, orig_fn, ret_type_kw, start_time_kw, *a, **kw):
        expr = getattr(self, "_corpus_expr", None)
        args = {"ctor_start": ctor_start_of(self)}
        s_start, tzf = effective_position(self, start_time_kw)
        effective_ret_type = ret_type_kw or getattr(self, "_ret_type", float)
        ret_str = (
            "datetime"
            if isinstance(effective_ret_type, type) and issubclass(effective_ret_type, datetime.datetime)
            else "float"
        )
        depth[0] += 1
        try:
            result = orig_fn(self, *a, **kw)
        except Exception as exc:  # noqa: BLE001
            record(op, expr, s_start, tzf, ret_str, args, False, exc=exc)
            raise
        finally:
            depth[0] -= 1
        record(op, expr, s_start, tzf, ret_kind(result), args, True, value=jsonify(result))
        return result

    def wrapped_get_next(self, ret_type=None, start_time=None, update_current=True):
        return _call_and_record(
            "next", self, orig_get_next, ret_type, start_time,
            ret_type=ret_type, start_time=start_time, update_current=update_current,
        )

    def wrapped_get_prev(self, ret_type=None, start_time=None, update_current=True):
        return _call_and_record(
            "prev", self, orig_get_prev, ret_type, start_time,
            ret_type=ret_type, start_time=start_time, update_current=update_current,
        )

    def wrapped_get_current(self, ret_type=None):
        expr = getattr(self, "_corpus_expr", None)
        args = {"ctor_start": ctor_start_of(self)}
        s_start, tzf = effective_position(self, None)
        depth[0] += 1
        try:
            result = orig_get_current(self, ret_type=ret_type)
        except Exception as exc:  # noqa: BLE001
            record("current", expr, s_start, tzf, "float", args, False, exc=exc)
            raise
        finally:
            depth[0] -= 1
        record("current", expr, s_start, tzf, ret_kind(result), args, True, value=jsonify(result))
        return result

    def wrapped_set_current(self, start_time, force=True):
        if depth[0] > 0:  # nested/internal call (e.g. from _get_next), not a real test-driven op
            return orig_set_current(self, start_time, force=force)
        expr = getattr(self, "_corpus_expr", None)
        args = {"ctor_start": ctor_start_of(self), "force": force}
        s_start, tzf = effective_position(self, start_time)
        depth[0] += 1
        try:
            result = orig_set_current(self, start_time, force=force)
        except Exception as exc:  # noqa: BLE001
            record("current", expr, s_start, tzf, "float", args, False, exc=exc)
            raise
        finally:
            depth[0] -= 1
        record("current", expr, s_start, tzf, "float", args, True, value=jsonify(result))
        return result

    def _wrap_all(op, orig_fn):
        def wrapped(self, ret_type=None, start_time=None, update_current=None):
            expr = getattr(self, "_corpus_expr", None)
            args = {"ctor_start": ctor_start_of(self)}
            s_start, tzf = effective_position(self, start_time)
            gen = orig_fn(self, ret_type=ret_type, start_time=start_time, update_current=update_current)
            values = []
            rec = {
                "op": op,
                "expr": expr,
                "start": s_start,
                "ret": "float",
                "args": args,
                "n": 0,
                "expect": {"ok": True, "value": values},
            }
            rec.update(tzf)
            RECORDS.append(rec)
            try:
                while True:
                    depth[0] += 1
                    try:
                        item = next(gen)
                    finally:
                        depth[0] -= 1
                    values.append(jsonify(item))
                    rec["ret"] = ret_kind(item)
                    rec["n"] = len(values)
                    yield item
            except StopIteration:
                return
            except Exception as exc:  # noqa: BLE001
                rec["n"] = len(values)
                rec["expect"] = {"ok": False, "error": type(exc).__name__, "message": str(exc)}
                raise

        return wrapped

    def wrapped_range(start, stop, expr_format, ret_type=None, **kwargs):
        s_start = iso_or_none(start) or datetime.datetime.now().isoformat()
        s_stop = iso_or_none(stop)
        tz_source = start.tzinfo if isinstance(start, datetime.datetime) else None
        tzf = describe_tz(tz_source)
        args = {k: v for k, v in kwargs.items() if k != "_croniter"}
        args["stop"] = s_stop
        gen = orig_range(start, stop, expr_format, ret_type=ret_type, **kwargs)
        values = []
        rec = {
            "op": "range",
            "expr": expr_format,
            "start": s_start,
            "ret": "float",
            "args": args,
            "expect": {"ok": True, "value": values},
        }
        rec.update(tzf)
        RECORDS.append(rec)
        try:
            while True:
                depth[0] += 1
                try:
                    item = next(gen)
                finally:
                    depth[0] -= 1
                values.append(jsonify(item))
                rec["ret"] = ret_kind(item)
                yield item
        except StopIteration:
            return
        except Exception as exc:  # noqa: BLE001
            rec["expect"] = {"ok": False, "error": type(exc).__name__, "message": str(exc)}
            raise

    # store expr_format / raw constructor start_time on instances so later
    # ops on the same object can report `expr` and `args.ctor_start`
    def init_and_stash(self, expr_format, *a, **kw):
        start_time = a[0] if a else kw.get("start_time")
        try:
            wrapped_init(self, expr_format, *a, **kw)
        finally:
            self._corpus_expr = expr_format
            self._corpus_ctor_start = iso_or_none(start_time)

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
    null_expr_or_start = sum(1 for r in deduped if r.get("expr") is None or r.get("start") is None)
    print(
        f"\ncorpus_plugin: wrote {len(deduped)} records ({len(RECORDS)} before dedup) to {OUT_PATH}"
        f" | null expr/start: {null_expr_or_start}"
    )
