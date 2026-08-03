//! Replays the golden corpus extracted from the original Python test suite.
//!
//! Every record is one call the original suite actually made, together with what the
//! Python returned or raised. Python is never invoked here: the corpus is committed data,
//! regenerated offline by `tools/extract_corpus/run.sh`. This is what keeps the port
//! honest without linking the source runtime.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use chrono::{DateTime, FixedOffset, NaiveDateTime, TimeZone};
use chrono_tz::Tz;
use croniter::{
    Croniter, CroniterError, Occurrence, Options, RetType, croniter_range, croniter_range_tz,
};
use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Deserialize)]
struct Record {
    op: String,
    expr: String,
    start: String,
    #[serde(default)]
    tz: Option<String>,
    #[serde(default)]
    tz_kind: Option<String>,
    #[serde(default)]
    ret: Option<String>,
    #[serde(default)]
    n: Option<usize>,
    #[serde(default)]
    args: Args,
    expect: Expect,
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
    /// Deliberately untyped: one record in the corpus passes a dict here, because the
    /// original suite checks that a non-str/bytes `hash_id` raises TypeError.
    #[serde(default)]
    hash_id: Option<Value>,
    #[serde(default)]
    exclude_ends: bool,
    #[serde(default)]
    stop: Option<String>,
    #[serde(default = "yes")]
    update_current: bool,
    /// Present only when the suite reassigned croniter's `_max_years_between_matches`
    /// attribute after construction, which bounds the search without marking it explicit.
    #[serde(default)]
    state_max_years_between_matches: Option<i64>,
}

impl Args {
    fn hash_id_bytes(&self) -> Option<&[u8]> {
        self.hash_id.as_ref()?.as_str().map(|s| s.as_bytes())
    }
}

fn yes() -> bool {
    true
}

#[derive(Debug, Deserialize)]
struct Expect {
    ok: bool,
    #[serde(default)]
    value: Option<Value>,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    message: Option<String>,
}

fn corpus_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/port/corpus.json")
}

/// Split an ISO-8601 string into its local part and its UTC offset, if it carries one.
///
/// Splitting on `['+', 'Z']` is not enough: a negative offset (`...T21:00:00-02:00`) has
/// no `+` and no `Z`, so the whole string survives the split and then fails to parse,
/// which the caller sees as an unparseable start rather than as a timezone it mishandled.
/// The offset also cannot simply be discarded — for a local time inside a DST fold it is
/// the only thing that says which of the two instants Python meant.
fn split_offset(s: &str) -> (String, Option<FixedOffset>) {
    let normalized = s.replace(' ', "T");
    if let Some(rest) = normalized.strip_suffix('Z') {
        return (rest.to_string(), FixedOffset::east_opt(0));
    }
    // An offset is the trailing `±HH:MM`; the `-` in the date never sits at that index.
    let bytes = normalized.as_bytes();
    if normalized.len() > 6 {
        let split_at = normalized.len() - 6;
        let sign = bytes[split_at];
        if (sign == b'+' || sign == b'-') && bytes[split_at + 3] == b':' {
            let (local, off) = normalized.split_at(split_at);
            if let Ok(parsed) = off[1..]
                .split(':')
                .try_fold(0i32, |acc, part| part.parse::<i32>().map(|n| acc * 60 + n))
            {
                let secs = parsed * 60;
                let signed = if sign == b'-' { -secs } else { secs };
                return (local.to_string(), FixedOffset::east_opt(signed));
            }
        }
    }
    (normalized, None)
}

fn parse_naive(s: &str) -> Option<NaiveDateTime> {
    let (local, _) = split_offset(s);
    NaiveDateTime::parse_from_str(&local, "%Y-%m-%dT%H:%M:%S%.f")
        .or_else(|_| NaiveDateTime::parse_from_str(&local, "%Y-%m-%dT%H:%M:%S"))
        .ok()
}

/// A fixed UTC offset expressed as a `chrono_tz::Tz`, which is what `Croniter` stores.
///
/// `Etc/GMT-N` is a zone with a constant +N:00 offset and no DST rules, so for the
/// whole-hour offsets the corpus actually contains this is an exact representation, not
/// an approximation. (The sign really is inverted; that is POSIX, not a typo.) Anything
/// that is not a whole hour returns `None` rather than silently rounding.
fn tz_for_fixed_offset(offset: FixedOffset) -> Option<Tz> {
    let secs = offset.local_minus_utc();
    if secs % 3600 != 0 {
        return None;
    }
    let hours = secs / 3600;
    if hours == 0 {
        return Some(Tz::UTC);
    }
    if !(-14..=12).contains(&hours) {
        return None;
    }
    format!(
        "Etc/GMT{}{}",
        if hours > 0 { '-' } else { '+' },
        hours.abs()
    )
    .parse()
    .ok()
}

/// Resolve a record's `start` to the timezone-aware instant Python was actually at.
fn aware_start(rec: &Record) -> Result<Option<DateTime<Tz>>, CroniterError> {
    let (local, offset) = split_offset(&rec.start);
    let naive = parse_naive(&local)
        .ok_or_else(|| CroniterError::Other(format!("unparseable start {:?}", rec.start)))?;

    match (rec.tz.as_deref(), offset) {
        // A named zone. When the record also carries an offset, go through the instant so
        // an ambiguous local time lands on the same side of the fold Python chose;
        // `.earliest()` would silently pick the first.
        (Some(name), off) => {
            let tz: Tz = name
                .parse()
                .map_err(|_| CroniterError::Other(format!("unknown tz {name:?}")))?;
            let aware = match off {
                Some(off) => off
                    .from_local_datetime(&naive)
                    .single()
                    .ok_or_else(|| CroniterError::Other("start not representable".into()))?
                    .with_timezone(&tz),
                None => naive
                    .and_local_timezone(tz)
                    .earliest()
                    .ok_or_else(|| CroniterError::Other("start not representable in tz".into()))?,
            };
            Ok(Some(aware))
        }
        // No named zone, but `tzinfo` was a plain fixed offset.
        (None, Some(off)) if rec.tz_kind.as_deref() == Some("fixed_offset") => {
            let tz = tz_for_fixed_offset(off)
                .ok_or_else(|| CroniterError::Other(format!("unmappable offset {off}")))?;
            let aware = naive
                .and_local_timezone(tz)
                .earliest()
                .ok_or_else(|| CroniterError::Other("start not representable in tz".into()))?;
            Ok(Some(aware))
        }
        _ => Ok(None),
    }
}

fn options_for(rec: &Record) -> Options {
    Options {
        ret_type: match rec.ret.as_deref() {
            Some("datetime") => RetType::DateTime,
            _ => RetType::Timestamp,
        },
        day_or: rec.args.day_or,
        max_years_between_matches: rec.args.max_years_between_matches,
        is_prev: false,
        hash_id: rec.args.hash_id_bytes().map(|h| h.to_vec()),
        implement_cron_bug: rec.args.implement_cron_bug,
        second_at_beginning: rec.args.second_at_beginning,
        expand_from_start_time: rec.args.expand_from_start_time,
    }
}

fn build(rec: &Record) -> Result<Croniter, CroniterError> {
    let mut cron = build_at_start(rec)?;
    if let Some(years) = rec.args.state_max_years_between_matches {
        cron.set_max_years_between_matches(years);
    }
    Ok(cron)
}

fn build_at_start(rec: &Record) -> Result<Croniter, CroniterError> {
    match aware_start(rec)? {
        Some(aware) => Croniter::with_options(&rec.expr, None, Some(aware), options_for(rec)),
        None => {
            let start = parse_naive(&rec.start).ok_or_else(|| {
                CroniterError::Other(format!("unparseable start {:?}", rec.start))
            })?;
            Croniter::with_options(&rec.expr, Some(start), None, options_for(rec))
        }
    }
}

/// Render an occurrence the way the corpus encodes it, so comparison is textual and
/// float/datetime formatting differences surface loudly instead of silently.
fn render(occ: Occurrence) -> Value {
    match occ {
        Occurrence::Timestamp(t) => Value::from(t),
        // Python's `isoformat()` prints microseconds only when they are non-zero, and
        // `get_current` hands back the cursor unrounded -- so a start of `...:59.999999`
        // must come back with its fraction intact, not truncated to the second.
        Occurrence::Naive(d) => Value::from(d.format("%Y-%m-%dT%H:%M:%S%.f").to_string()),
        Occurrence::DateTime(d) => Value::from(d.format("%Y-%m-%dT%H:%M:%S%.f%:z").to_string()),
    }
}

/// Compare loosely enough to tolerate float formatting, strictly enough to catch a wrong
/// instant. Timestamps must agree to the microsecond; strings must agree after trimming
/// a trailing all-zero fractional part.
fn value_matches(expected: &Value, got: &Value) -> bool {
    match (expected, got) {
        (Value::Number(a), Value::Number(b)) => match (a.as_f64(), b.as_f64()) {
            (Some(x), Some(y)) => (x - y).abs() < 1e-6,
            _ => a == b,
        },
        (Value::String(a), Value::String(b)) => normalize_dt(a) == normalize_dt(b),
        (Value::Array(a), Value::Array(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(x, y)| value_matches(x, y))
        }
        _ => expected == got,
    }
}

fn normalize_dt(s: &str) -> String {
    let s = s.replace(' ', "T");
    let s = s.trim_end_matches('0').trim_end_matches('.').to_string();
    // "+00:00" and "Z" denote the same offset.
    s.replace("+00:00", "Z")
}

enum Outcome {
    Value(Value),
    Error(String),
}

/// Render a `range` result the way the record recorded it.
///
/// `croniter_range` picks its return type from the *bounds* it was given: numeric bounds
/// come back as timestamps (croniter.py:1448-1452, `auto_rt = float`), datetimes come back
/// as datetimes. The corpus preserves that, so the replay has to read `ret` rather than
/// always formatting a string.
fn finish(items: Vec<Occurrence>, rec: &Record) -> Result<Value, CroniterError> {
    let as_float = rec.ret.as_deref() == Some("float");
    Ok(Value::Array(
        items
            .into_iter()
            .map(|occ| {
                if as_float {
                    Value::from(occ.as_timestamp())
                } else {
                    render(occ)
                }
            })
            .collect(),
    ))
}

fn run_record(rec: &Record) -> Outcome {
    let ret = match rec.ret.as_deref() {
        Some("datetime") => Some(RetType::DateTime),
        Some("float") => Some(RetType::Timestamp),
        _ => None,
    };

    let result: Result<Value, CroniterError> = (|| {
        match rec.op.as_str() {
            // `validate` records come from two different Python call shapes: an actual
            // `croniter.is_valid(...)` returning a bool, and a bare `croniter(expr)`
            // construction whose only assertion is which exception it raises. The
            // recorded `expect` tells them apart, so replay has to as well: comparing a
            // raise against `is_valid`'s `false` would mark a correct port as wrong.
            "validate" => {
                let parsed = croniter::expand::expand(
                    &rec.expr,
                    rec.args.hash_id_bytes(),
                    rec.args.second_at_beginning,
                    None,
                    None,
                );
                if rec.expect.ok {
                    Ok(Value::from(parsed.is_ok()))
                } else {
                    parsed.map(|_| Value::from(true))
                }
            }
            "current" => {
                let cron = build(rec)?;
                Ok(render(cron.get_current(ret)))
            }
            "next" => {
                let mut cron = build(rec)?;
                Ok(render(cron.get_next(ret)?))
            }
            "prev" => {
                let mut cron = build(rec)?;
                Ok(render(cron.get_prev(ret)?))
            }
            "all_next" | "all_prev" => {
                let mut cron = build(rec)?;
                let n = rec.n.unwrap_or_else(|| {
                    rec.expect
                        .value
                        .as_ref()
                        .and_then(|v| v.as_array())
                        .map(|a| a.len())
                        .unwrap_or(1)
                });
                // `n` counts what the Python generator *yielded*. When it raised on the
                // very first step that is zero, and asking the port for zero items would
                // trivially succeed with an empty list instead of reproducing the raise.
                let n = if rec.expect.ok { n } else { n.max(1) };
                let items = cron.all_from(ret, n, rec.op == "all_prev", rec.args.update_current)?;
                Ok(Value::Array(items.into_iter().map(render).collect()))
            }
            "range" => {
                let stop_raw = rec
                    .args
                    .stop
                    .as_deref()
                    .ok_or_else(|| CroniterError::Other("range requires stop".into()))?;

                // A range over aware bounds keeps its offsets all the way through, so it
                // has to be walked in the named zone rather than flattened to local time
                // -- these are the DST-crossing cases, where the two differ.
                if let Some(from_aware) = aware_start(rec)? {
                    let tz = from_aware.timezone();
                    let (stop_local, stop_off) = split_offset(stop_raw);
                    let stop_naive = parse_naive(&stop_local)
                        .ok_or_else(|| CroniterError::Other("bad range stop".into()))?;
                    let to_aware = match stop_off {
                        Some(off) => off
                            .from_local_datetime(&stop_naive)
                            .single()
                            .ok_or_else(|| CroniterError::Other("bad range stop".into()))?
                            .with_timezone(&tz),
                        None => stop_naive
                            .and_local_timezone(tz)
                            .earliest()
                            .ok_or_else(|| CroniterError::Other("bad range stop".into()))?,
                    };
                    let items = croniter_range_tz(
                        from_aware,
                        to_aware,
                        &rec.expr,
                        rec.args.day_or,
                        rec.args.exclude_ends,
                        rec.args.second_at_beginning,
                    )?;
                    return finish(items.into_iter().map(Occurrence::DateTime).collect(), rec);
                }

                let from = parse_naive(&rec.start)
                    .ok_or_else(|| CroniterError::Other("bad range start".into()))?;
                let to = parse_naive(stop_raw)
                    .ok_or_else(|| CroniterError::Other("bad range stop".into()))?;
                let items = croniter_range(
                    from,
                    to,
                    &rec.expr,
                    rec.args.day_or,
                    rec.args.exclude_ends,
                    rec.args.second_at_beginning,
                )?;
                finish(items.into_iter().map(Occurrence::Naive).collect(), rec)
            }
            other => Err(CroniterError::Other(format!("unsupported op {other:?}"))),
        }
    })();

    match result {
        Ok(v) => Outcome::Value(v),
        Err(e) => Outcome::Error(e.class_name().to_string()),
    }
}

/// Ops the corpus carries that the replay harness understands. Anything else is counted
/// as skipped rather than silently passing.
const SUPPORTED: [&str; 7] = [
    "validate", "current", "next", "prev", "all_next", "all_prev", "range",
];

/// Python-side failures the Rust API cannot reproduce, keyed by the message the corpus
/// recorded. Each is a call that simply does not typecheck or does not exist in this
/// port, not a behaviour it gets wrong — see DECISIONS.md §15. Matched on the exact
/// message so that a *different* error arising from the same call still fails loudly.
const UNREPRESENTABLE: [&str; 4] = [
    // `ret_type` is an enum here, so there is no third value to reject.
    "Invalid ret_type, only 'float' or 'datetime' is acceptable.",
    // `hash_id` is `Option<Vec<u8>>`; the suite passes a dict to prove Python rejects it.
    "hash_id must be bytes or UTF-8 string",
    // `croniter_range` takes two `NaiveDateTime`s, so start and stop cannot disagree.
    "The start and stop must be same type.",
    // `get_next` takes no `start_time`, so the combination it guards against is unbuildable.
    "start_time is not supported when using expand_from_start_time = True.",
];

fn is_unrepresentable(rec: &Record) -> bool {
    if rec.expect.ok {
        return false;
    }
    rec.expect
        .message
        .as_deref()
        .is_some_and(|m| UNREPRESENTABLE.iter().any(|u| m.starts_with(u)))
}

/// Does any field of `expr` use croniter's random syntax (`R`, `R(a-b)`)?
///
/// Matched per-field on the whole token so that `FRI`, `MAR` and friends — which merely
/// contain an `R` — are left alone.
fn is_random_expr(expr: &str) -> bool {
    expr.split_whitespace().any(|field| {
        field.split(',').any(|part| {
            let part = part.trim();
            part == "R" || (part.starts_with("R(") && part.ends_with(')'))
        })
    })
}

#[test]
fn corpus_replays_against_the_port() {
    let raw = fs::read_to_string(corpus_path())
        .expect("tests/port/corpus.json missing. Regenerate it with tools/extract_corpus/run.sh");
    let records: Vec<Record> = serde_json::from_str(&raw).expect("corpus is not valid JSON");
    assert!(!records.is_empty(), "corpus is empty");

    // A capped dump turns one red run into several: you fix the first 25, rerun, and
    // meet the next 25. Default high enough to see a whole regression at once.
    let dump_limit: usize = std::env::var("CORPUS_DUMP_LIMIT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(200);

    let mut passed = 0usize;
    let mut skipped = 0usize;
    let mut random_skipped = 0usize;
    let mut unrepresentable = 0usize;
    let mut failures: Vec<String> = Vec::new();
    let mut failures_by_op: BTreeMap<String, usize> = BTreeMap::new();

    for (i, rec) in records.iter().enumerate() {
        if !SUPPORTED.contains(&rec.op.as_str()) {
            skipped += 1;
            continue;
        }
        // `R` / `R(a-b)` expand to a *random* member of the field's range at parse time
        // (croniter.py:1587-1620). Python recorded one draw; the port makes its own. There
        // is no answer these records could be checked against, so they are excluded rather
        // than counted as passes — see DECISIONS.md §12. Anything else with an `R` in it
        // (`FRI`, `MAR`, ...) is alphabetic and unaffected.
        if is_random_expr(&rec.expr) {
            random_skipped += 1;
            continue;
        }
        if is_unrepresentable(rec) {
            unrepresentable += 1;
            continue;
        }

        let got = run_record(rec);
        let ok = match (&got, rec.expect.ok) {
            (Outcome::Value(v), true) => match &rec.expect.value {
                Some(expected) => value_matches(expected, v),
                None => true,
            },
            (Outcome::Error(class), false) => match &rec.expect.error {
                Some(expected) => expected == class,
                None => true,
            },
            _ => false,
        };

        if ok {
            passed += 1;
        } else {
            *failures_by_op.entry(rec.op.clone()).or_default() += 1;
            if failures.len() < dump_limit {
                let detail = match &got {
                    Outcome::Value(v) => format!("got value {v}"),
                    Outcome::Error(e) => format!("got error {e}"),
                };
                failures.push(format!(
                    "#{i} op={} expr={:?} start={:?} tz={:?}\n     expected ok={} value={:?} error={:?}\n     {detail}",
                    rec.op,
                    rec.expr,
                    rec.start,
                    rec.tz,
                    rec.expect.ok,
                    rec.expect.value,
                    rec.expect.error,
                ));
            }
        }
    }

    let attempted = records.len() - skipped - random_skipped - unrepresentable;
    let failed = attempted - passed;
    let rate = if attempted == 0 {
        0.0
    } else {
        passed as f64 / attempted as f64 * 100.0
    };

    println!(
        "corpus: {passed}/{attempted} passed ({rate:.2}%), {failed} failed, \
         {skipped} unsupported-op, {random_skipped} random-expr (unverifiable), \
         {unrepresentable} unrepresentable-in-rust, {} total",
        records.len()
    );
    if !failures_by_op.is_empty() {
        println!("failures by op: {failures_by_op:?}");
    }

    assert!(
        failed == 0,
        "{failed} of {attempted} corpus records diverged from Python.\nFirst {} failures:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
