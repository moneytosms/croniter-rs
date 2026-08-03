//! Replays the golden corpus extracted from the original Python test suite.
//!
//! Every record is one call the original suite actually made, together with what the
//! Python returned or raised. Python is never invoked here: the corpus is committed data,
//! regenerated offline by `tools/extract_corpus/run.sh`. This is what keeps the port
//! honest without linking the source runtime.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use chrono::{DateTime, NaiveDateTime};
use chrono_tz::Tz;
use croniter::{Croniter, CroniterError, Occurrence, Options, RetType, croniter_range};
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
}

fn corpus_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/port/corpus.json")
}

fn parse_naive(s: &str) -> Option<NaiveDateTime> {
    let normalized = s.replace(' ', "T");
    let trimmed = normalized.split(['+', 'Z']).next()?.to_string();
    NaiveDateTime::parse_from_str(&trimmed, "%Y-%m-%dT%H:%M:%S%.f")
        .or_else(|_| NaiveDateTime::parse_from_str(&trimmed, "%Y-%m-%dT%H:%M:%S"))
        .ok()
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
    let start = parse_naive(&rec.start)
        .ok_or_else(|| CroniterError::Other(format!("unparseable start {:?}", rec.start)))?;
    match rec.tz.as_deref() {
        Some(name) => {
            let tz: Tz = name
                .parse()
                .map_err(|_| CroniterError::Other(format!("unknown tz {name:?}")))?;
            let aware: DateTime<Tz> = start
                .and_local_timezone(tz)
                .earliest()
                .ok_or_else(|| CroniterError::Other("start not representable in tz".into()))?;
            Croniter::with_options(&rec.expr, None, Some(aware), options_for(rec))
        }
        None => Croniter::with_options(&rec.expr, Some(start), None, options_for(rec)),
    }
}

/// Render an occurrence the way the corpus encodes it, so comparison is textual and
/// float/datetime formatting differences surface loudly instead of silently.
fn render(occ: Occurrence) -> Value {
    match occ {
        Occurrence::Timestamp(t) => Value::from(t),
        Occurrence::Naive(d) => Value::from(d.format("%Y-%m-%dT%H:%M:%S").to_string()),
        Occurrence::DateTime(d) => Value::from(d.format("%Y-%m-%dT%H:%M:%S%:z").to_string()),
    }
}

/// Compare loosely enough to tolerate float formatting, strictly enough to catch a wrong
/// instant. Timestamps must agree to the microsecond; strings must agree after trimming
/// a trailing all-zero fractional part.
fn value_matches(expected: &Value, got: &Value) -> bool {
    match (expected, got) {
        (Value::Number(a), Value::Number(b)) => {
            match (a.as_f64(), b.as_f64()) {
                (Some(x), Some(y)) => (x - y).abs() < 1e-6,
                _ => a == b,
            }
        }
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

fn run_record(rec: &Record) -> Outcome {
    let ret = match rec.ret.as_deref() {
        Some("datetime") => Some(RetType::DateTime),
        Some("float") => Some(RetType::Timestamp),
        _ => None,
    };

    let result: Result<Value, CroniterError> = (|| {
        match rec.op.as_str() {
            "validate" => Ok(Value::from(Croniter::is_valid(
                &rec.expr,
                rec.args.hash_id_bytes(),
                rec.args.second_at_beginning,
            ))),
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
                let items = if rec.op == "all_next" {
                    cron.all_next(ret, n)?
                } else {
                    cron.all_prev(ret, n)?
                };
                Ok(Value::Array(items.into_iter().map(render).collect()))
            }
            "range" => {
                let from = parse_naive(&rec.start)
                    .ok_or_else(|| CroniterError::Other("bad range start".into()))?;
                let to = rec
                    .args
                    .stop
                    .as_deref()
                    .and_then(parse_naive)
                    .ok_or_else(|| CroniterError::Other("bad range stop".into()))?;
                let items = croniter_range(
                    from,
                    to,
                    &rec.expr,
                    rec.args.day_or,
                    rec.args.exclude_ends,
                )?;
                Ok(Value::Array(
                    items
                        .into_iter()
                        .map(|d| Value::from(d.format("%Y-%m-%dT%H:%M:%S").to_string()))
                        .collect(),
                ))
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

#[test]
fn corpus_replays_against_the_port() {
    let raw = fs::read_to_string(corpus_path()).expect(
        "tests/port/corpus.json missing. Regenerate it with tools/extract_corpus/run.sh",
    );
    let records: Vec<Record> = serde_json::from_str(&raw).expect("corpus is not valid JSON");
    assert!(!records.is_empty(), "corpus is empty");

    let mut passed = 0usize;
    let mut skipped = 0usize;
    let mut failures: Vec<String> = Vec::new();
    let mut failures_by_op: BTreeMap<String, usize> = BTreeMap::new();

    for (i, rec) in records.iter().enumerate() {
        if !SUPPORTED.contains(&rec.op.as_str()) || rec.tz_kind.is_some() {
            skipped += 1;
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
            if failures.len() < 25 {
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

    let attempted = records.len() - skipped;
    let failed = attempted - passed;
    let rate = if attempted == 0 {
        0.0
    } else {
        passed as f64 / attempted as f64 * 100.0
    };

    println!(
        "corpus: {passed}/{attempted} passed ({rate:.2}%), {failed} failed, {skipped} skipped, {} total",
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
