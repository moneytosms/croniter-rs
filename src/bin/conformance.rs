//! Conformance server: line-delimited JSON on stdin/stdout.
//!
//! Exists so the byte-identical original Python test suite can be run against this port
//! without the port ever linking to Python. A pure-Python shim under `tools/bridge/`
//! satisfies `import croniter` and forwards each call here over a pipe.
//!
//! Hackathon Rule 05 forbids the *port* linking or FFI-ing into the source runtime. This
//! is the inverse: Python spawns the port as an ordinary subprocess. The crate contains
//! no Python, and `cargo test` passes with no Python installed.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::io::{self, BufRead, Write};

use chrono::{DateTime, NaiveDateTime};
use chrono_tz::Tz;
use croniter::{Croniter, CroniterError, Expanded, Occurrence, Options, RetType, croniter_range};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Debug, Deserialize)]
struct Request {
    id: u64,
    op: String,
    expr: String,
    #[serde(default)]
    start: Option<String>,
    #[serde(default)]
    tz: Option<String>,
    #[serde(default)]
    ret: Option<String>,
    #[serde(default)]
    n: Option<usize>,
    #[serde(default)]
    stop: Option<String>,
    #[serde(default)]
    args: Args,
    /// Identifies a parse held on this side for the lifetime of one Python `croniter`.
    ///
    /// Every other field of a request is a pure function of its inputs, so rebuilding
    /// from `expr` each time is equivalent, except for `R` (random) expressions, whose
    /// expansion croniter draws once in `__init__` and then reuses. Re-expanding those
    /// per call makes consecutive `get_next()`s wander instead of advancing. The shim
    /// creates a handle in its constructor and quotes it thereafter.
    #[serde(default)]
    handle: Option<u64>,
}

#[derive(Debug, Default, Deserialize)]
struct Args {
    #[serde(default = "yes")]
    day_or: bool,
    #[serde(default)]
    second_at_beginning: bool,
    #[serde(default)]
    implement_cron_bug: bool,
    #[serde(default)]
    expand_from_start_time: bool,
    #[serde(default)]
    max_years_between_matches: Option<i64>,
    /// Hex-encoded. `hash_id` is arbitrary bytes that croniter feeds to a hash, not
    /// text, and the original suite passes byte strings that are not valid UTF-8 -- so
    /// it cannot travel through JSON as a string without either corrupting or raising.
    #[serde(default)]
    hash_id_hex: Option<String>,
    #[serde(default)]
    exclude_ends: bool,
    /// `expand`-only: the instant a start-time-relative expansion anchors to.
    /// `*/15` means "every 15 seconds from :00" normally, but "every 15 seconds from
    /// the start second" when `expand_from_start_time` is on, so the expansion cannot
    /// be computed without it.
    #[serde(default)]
    from_timestamp: Option<f64>,
    #[serde(default)]
    from_timestamp_tz: Option<String>,
    /// `validate`-only: croniter's opt-in cross-validation of day-of-month against
    /// month (and year), which rejects things like Feb 31st.
    #[serde(default)]
    strict: bool,
    #[serde(default)]
    strict_year: Vec<i64>,
}

impl Args {
    fn hash_id(&self) -> Result<Option<Vec<u8>>, CroniterError> {
        let Some(hex) = self.hash_id_hex.as_deref() else {
            return Ok(None);
        };
        if !hex.len().is_multiple_of(2) {
            return Err(CroniterError::Other(format!(
                "odd-length hash_id_hex {hex:?}"
            )));
        }
        (0..hex.len())
            .step_by(2)
            .map(|i| {
                u8::from_str_radix(&hex[i..i + 2], 16)
                    .map_err(|e| CroniterError::Other(format!("bad hash_id_hex {hex:?}: {e}")))
            })
            .collect::<Result<Vec<u8>, _>>()
            .map(Some)
    }
}

fn yes() -> bool {
    true
}

#[derive(Debug, Serialize)]
struct Failure {
    id: u64,
    ok: bool,
    error: String,
    message: String,
}

fn main() -> io::Result<()> {
    let stdin = io::stdin();
    let mut stdout = io::stdout();

    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let response = match serde_json::from_str::<Request>(&line) {
            Ok(req) => {
                let id = req.id;
                match handle(req) {
                    Ok(value) => json!({ "id": id, "ok": true, "value": value }),
                    Err(e) => serde_json::to_value(Failure {
                        id,
                        ok: false,
                        error: e.class_name().to_string(),
                        message: e.to_string(),
                    })
                    .unwrap_or_else(|_| json!({ "id": id, "ok": false, "error": "CroniterError" })),
                }
            }
            Err(e) => json!({
                "id": Value::Null,
                "ok": false,
                "error": "ProtocolError",
                "message": e.to_string(),
            }),
        };
        writeln!(stdout, "{response}")?;
        stdout.flush()?;
    }
    Ok(())
}

/// Split an ISO-8601 string into its local part and its UTC offset, if it has one.
///
/// Splitting on `['+', 'Z']` misses negative offsets: `2018-02-17T21:00:00-02:00` has
/// neither, so the whole string survives and then fails to parse. The offset is also
/// worth keeping rather than discarding, because inside a DST fold it is the only thing that
/// says which of the two instants the caller meant.
fn split_offset(s: &str) -> (&str, Option<chrono::FixedOffset>) {
    if let Some(rest) = s.strip_suffix('Z') {
        return (rest, chrono::FixedOffset::east_opt(0));
    }
    let bytes = s.as_bytes();
    if s.len() > 6 {
        let at = s.len() - 6;
        if (bytes[at] == b'+' || bytes[at] == b'-') && bytes[at + 3] == b':' {
            let (local, off) = s.split_at(at);
            let mins = off[1..]
                .split(':')
                .try_fold(0i32, |acc, part| part.parse::<i32>().map(|n| acc * 60 + n));
            if let Ok(mins) = mins {
                let secs = if bytes[at] == b'-' {
                    -mins * 60
                } else {
                    mins * 60
                };
                return (local, chrono::FixedOffset::east_opt(secs));
            }
        }
    }
    (s, None)
}

fn parse_naive(s: &str) -> Result<NaiveDateTime, CroniterError> {
    // Accept both "YYYY-MM-DDTHH:MM:SS[.ffffff]" and the space-separated form Python's
    // str(datetime) produces.
    let normalized = s.replace(' ', "T");
    let (trimmed, _) = split_offset(&normalized);
    NaiveDateTime::parse_from_str(trimmed, "%Y-%m-%dT%H:%M:%S%.f")
        .or_else(|_| NaiveDateTime::parse_from_str(trimmed, "%Y-%m-%dT%H:%M:%S"))
        .map_err(|e| CroniterError::Other(format!("bad datetime {s:?}: {e}")))
}

/// The instant a request's `start` denotes, honouring the offset it carries.
fn parse_aware(s: &str, tz: Tz) -> Result<DateTime<Tz>, CroniterError> {
    use chrono::TimeZone;
    let normalized = s.replace(' ', "T");
    let (local, offset) = split_offset(&normalized);
    let naive = NaiveDateTime::parse_from_str(local, "%Y-%m-%dT%H:%M:%S%.f")
        .or_else(|_| NaiveDateTime::parse_from_str(local, "%Y-%m-%dT%H:%M:%S"))
        .map_err(|e| CroniterError::Other(format!("bad datetime {s:?}: {e}")))?;
    match offset {
        Some(off) => off
            .from_local_datetime(&naive)
            .single()
            .map(|dt| dt.with_timezone(&tz))
            .ok_or_else(|| CroniterError::Other(format!("bad datetime {s:?}"))),
        None => Ok(resolve_local(naive, tz)),
    }
}

/// A local wall-clock time as an instant, including times that do not exist.
///
/// During a spring-forward gap `and_local_timezone` yields `LocalResult::None`, and
/// erroring there is wrong: Python builds the datetime regardless and the offset in
/// force *before* the transition is what it ends up meaning, so 02:05 on a US
/// spring-forward day reads back as 03:05 local. Rejecting it would make the port look
/// as though it could not handle the very transitions it is being tested on.
fn resolve_local(naive: NaiveDateTime, tz: Tz) -> DateTime<Tz> {
    use chrono::{Duration, TimeZone};
    if let Some(dt) = naive.and_local_timezone(tz).earliest() {
        return dt;
    }
    // Inside the gap: interpret against the pre-transition offset. A gap is at most a
    // few hours, so stepping back to a resolvable local time and re-adding the delta
    // lands on the instant Python names.
    for minutes in 1..=(24 * 60) {
        let probe = naive - Duration::minutes(minutes);
        if let Some(before) = probe.and_local_timezone(tz).earliest() {
            return before + Duration::minutes(minutes);
        }
    }
    tz.from_utc_datetime(&naive)
}

fn resolve_tz(name: &str) -> Result<Tz, CroniterError> {
    name.parse::<Tz>()
        .map_err(|_| CroniterError::Other(format!("unknown timezone {name:?}")))
}

// Parses held on behalf of live Python `croniter` objects, keyed by handle.
//
// Single-threaded: the protocol is one request per line on stdin, answered before the
// next is read, so a plain `RefCell` in thread-local storage is enough and avoids a
// mutex that would never be contended.
thread_local! {
    static PARSES: RefCell<HashMap<u64, Expanded>> = RefCell::new(HashMap::new());
    static NEXT_HANDLE: Cell<u64> = const { Cell::new(1) };
}

fn store_parse(expanded: Expanded) -> u64 {
    let handle = NEXT_HANDLE.with(|h| {
        let v = h.get();
        h.set(v + 1);
        v
    });
    PARSES.with(|p| p.borrow_mut().insert(handle, expanded));
    handle
}

fn take_parse(handle: u64) -> Option<Expanded> {
    PARSES.with(|p| p.borrow().get(&handle).cloned())
}

fn build_options(req: &Request) -> Result<Options, CroniterError> {
    Ok(Options {
        ret_type: match req.ret.as_deref() {
            Some("datetime") => RetType::DateTime,
            _ => RetType::Timestamp,
        },
        day_or: req.args.day_or,
        max_years_between_matches: req.args.max_years_between_matches,
        is_prev: false,
        hash_id: req.args.hash_id()?,
        implement_cron_bug: req.args.implement_cron_bug,
        second_at_beginning: req.args.second_at_beginning,
        expand_from_start_time: req.args.expand_from_start_time,
    })
}

/// Resolve a request's start into the (naive, aware) pair the constructors take.
fn start_pair(
    req: &Request,
) -> Result<(Option<NaiveDateTime>, Option<DateTime<Tz>>), CroniterError> {
    match (&req.tz, req.start.as_deref()) {
        (Some(tz_name), Some(start)) => Ok((None, Some(parse_aware(start, resolve_tz(tz_name)?)?))),
        (_, start) => Ok((start.map(parse_naive).transpose()?, None)),
    }
}

fn make_cron(req: &Request) -> Result<Croniter, CroniterError> {
    let opts = build_options(req)?;
    let (naive, aware) = start_pair(req)?;

    // A handle means the Python object this call belongs to already parsed its
    // expression, so reuse that parse rather than making a new one. For everything but
    // `R` the two are identical; for `R` only the stored one carries the draw croniter
    // committed to in `__init__`.
    if let Some(expanded) = req.handle.and_then(take_parse) {
        return Ok(Croniter::from_expanded(expanded, naive, aware, opts));
    }
    Croniter::with_options(&req.expr, naive, aware, opts)
}

fn encode(occ: Occurrence) -> Value {
    match occ {
        Occurrence::Timestamp(t) => json!(t),
        Occurrence::Naive(d) => json!(d.format("%Y-%m-%dT%H:%M:%S%.6f").to_string()),
        Occurrence::DateTime(d) => json!(d.format("%Y-%m-%dT%H:%M:%S%.6f%:z").to_string()),
    }
}

fn handle(req: Request) -> Result<Value, CroniterError> {
    match req.op.as_str() {
        // Propagates the parse error rather than reporting a bool.
        //
        // Both callers need the failure, not a verdict: the shim's `__init__` issues a
        // `validate` to reproduce croniter's eager `_expand()` (a bad expression has to
        // raise out of the constructor), and its `is_valid` wraps the same call in
        // `try/except` to turn that raise back into `False`. Answering `false` here would
        // be a successful response, so the constructor would happily build a croniter
        // around an expression Python rejects.
        "validate" => {
            let expanded = croniter::expand::expand(
                &req.expr,
                req.args.hash_id()?.as_deref(),
                req.args.second_at_beginning,
                None,
                None,
            )?;
            if req.args.strict {
                croniter::expand::check_strict(&expanded, &req.expr, &req.args.strict_year)?;
            }
            Ok(json!(true))
        }

        // Like `validate`, but keeps the resulting parse and names it. The shim's
        // `__init__` uses this so that one Python `croniter` maps to one parse here,
        // which is what croniter's own semantics require for `R` expressions.
        "create" => {
            let (naive, aware) = start_pair(&req)?;
            let start_ts = match (naive, aware) {
                (_, Some(a)) => Some(croniter::datetime_to_timestamp(a)),
                (Some(n), None) => Some(croniter::naive_to_timestamp(n)),
                (None, None) => None,
            };
            let anchor = req
                .args
                .expand_from_start_time
                .then_some(start_ts)
                .flatten();
            let tz = match (&req.tz, req.args.expand_from_start_time) {
                (Some(name), true) => Some(resolve_tz(name)?),
                _ => None,
            };
            let expanded = croniter::expand::expand(
                &req.expr,
                req.args.hash_id()?.as_deref(),
                req.args.second_at_beginning,
                anchor,
                tz,
            )?;
            if req.args.strict {
                croniter::expand::check_strict(&expanded, &req.expr, &req.args.strict_year)?;
            }
            Ok(json!(store_parse(expanded)))
        }

        // Python-side object went away; drop the parse so a long session does not grow
        // a handle map forever.
        "destroy" => {
            if let Some(h) = req.handle {
                PARSES.with(|p| p.borrow_mut().remove(&h));
            }
            Ok(json!(true))
        }

        "next" => {
            let mut cron = make_cron(&req)?;
            let ret = cron.ret_type_hint(req.ret.as_deref());
            Ok(encode(cron.get_next(ret)?))
        }

        "prev" => {
            let mut cron = make_cron(&req)?;
            let ret = cron.ret_type_hint(req.ret.as_deref());
            Ok(encode(cron.get_prev(ret)?))
        }

        "current" => {
            let cron = make_cron(&req)?;
            let ret = cron.ret_type_hint(req.ret.as_deref());
            Ok(encode(cron.get_current(ret)))
        }

        "all_next" | "all_prev" => {
            let mut cron = make_cron(&req)?;
            let ret = cron.ret_type_hint(req.ret.as_deref());
            let n = req.n.unwrap_or(1);
            let items = if req.op == "all_next" {
                cron.all_next(ret, n)?
            } else {
                cron.all_prev(ret, n)?
            };
            Ok(Value::Array(items.into_iter().map(encode).collect()))
        }

        "match" => {
            let start = req
                .start
                .as_deref()
                .map(parse_naive)
                .transpose()?
                .ok_or_else(|| CroniterError::Other("match requires start".into()))?;
            Ok(json!(Croniter::matches(&req.expr, start, req.args.day_or)?))
        }

        "match_range" => {
            let from = req
                .start
                .as_deref()
                .map(parse_naive)
                .transpose()?
                .ok_or_else(|| CroniterError::Other("match_range requires start".into()))?;
            let to = req
                .stop
                .as_deref()
                .map(parse_naive)
                .transpose()?
                .ok_or_else(|| CroniterError::Other("match_range requires stop".into()))?;
            Ok(json!(Croniter::match_range(
                &req.expr,
                from,
                to,
                req.args.day_or
            )?))
        }

        "range" => {
            let from = req
                .start
                .as_deref()
                .map(parse_naive)
                .transpose()?
                .ok_or_else(|| CroniterError::Other("range requires start".into()))?;
            let to = req
                .stop
                .as_deref()
                .map(parse_naive)
                .transpose()?
                .ok_or_else(|| CroniterError::Other("range requires stop".into()))?;
            let items = croniter_range(
                from,
                to,
                &req.expr,
                req.args.day_or,
                req.args.exclude_ends,
                req.args.second_at_beginning,
            )?;
            Ok(Value::Array(
                items
                    .into_iter()
                    .map(|d| json!(d.format("%Y-%m-%dT%H:%M:%S%.6f").to_string()))
                    .collect(),
            ))
        }

        // White-box op. 118 assertions in the original suite read the parse tree
        // directly (`.expanded` 68 times, `croniter.expand` 37, `HashExpander` 13), so
        // the bridge needs a way to hand it back in croniter's own shape.
        "expand" => {
            // Reading `.expanded` off an instance must show that instance's own parse.
            // Re-expanding an `R` expression here would report a different draw from the
            // one its `get_next` is using, or the object would disagree with itself.
            let expanded = match req.handle.and_then(take_parse) {
                Some(expanded) => expanded,
                None => {
                    let from_tz = req
                        .args
                        .from_timestamp_tz
                        .as_deref()
                        .map(resolve_tz)
                        .transpose()?;
                    croniter::expand::expand(
                        &req.expr,
                        req.args.hash_id()?.as_deref(),
                        req.args.second_at_beginning,
                        req.args.from_timestamp,
                        from_tz,
                    )?
                }
            };
            if req.args.strict {
                croniter::expand::check_strict(&expanded, &req.expr, &req.args.strict_year)?;
            }
            Ok(json!({
                "expanded": expanded
                    .fields
                    .iter()
                    .map(|field| {
                        Value::Array(
                            field
                                .iter()
                                .map(|e| match e {
                                    croniter::Expr::Star => json!("*"),
                                    croniter::Expr::Last => json!("l"),
                                    croniter::Expr::Num(n) => json!(n),
                                })
                                .collect(),
                        )
                    })
                    .collect::<Vec<_>>(),
                "nth_weekday_of_month": expanded
                    .nth_weekday_of_month
                    .iter()
                    .map(|(dow, set)| (dow.to_string(), set.iter().copied().collect::<Vec<_>>()))
                    .collect::<std::collections::BTreeMap<_, _>>(),
                "expressions": expanded.expressions,
                "nearest_weekday": expanded.nearest_weekday,
            }))
        }

        other => Err(CroniterError::Other(format!("unknown op {other:?}"))),
    }
}

/// Small helper so each arm does not repeat the `ret` string mapping.
trait RetHint {
    fn ret_type_hint(&self, ret: Option<&str>) -> Option<RetType>;
}

impl RetHint for Croniter {
    fn ret_type_hint(&self, ret: Option<&str>) -> Option<RetType> {
        match ret {
            Some("datetime") => Some(RetType::DateTime),
            Some("float") => Some(RetType::Timestamp),
            _ => None,
        }
    }
}
