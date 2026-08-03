//! Conformance server: line-delimited JSON on stdin/stdout.
//!
//! Exists so the byte-identical original Python test suite can be run against this port
//! without the port ever linking to Python. A pure-Python shim under `tools/bridge/`
//! satisfies `import croniter` and forwards each call here over a pipe.
//!
//! Hackathon Rule 05 forbids the *port* linking or FFI-ing into the source runtime. This
//! is the inverse: Python spawns the port as an ordinary subprocess. The crate contains
//! no Python, and `cargo test` passes with no Python installed.

use std::io::{self, BufRead, Write};

use chrono::{DateTime, NaiveDateTime};
use chrono_tz::Tz;
use croniter::{Croniter, CroniterError, Occurrence, Options, RetType, croniter_range};
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
    #[serde(default)]
    hash_id: Option<String>,
    #[serde(default)]
    exclude_ends: bool,
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

fn parse_naive(s: &str) -> Result<NaiveDateTime, CroniterError> {
    // Accept both "YYYY-MM-DDTHH:MM:SS[.ffffff]" and the space-separated form Python's
    // str(datetime) produces.
    let normalized = s.replace(' ', "T");
    let trimmed = normalized
        .split(['+', 'Z'])
        .next()
        .unwrap_or(&normalized)
        .to_string();
    NaiveDateTime::parse_from_str(&trimmed, "%Y-%m-%dT%H:%M:%S%.f")
        .or_else(|_| NaiveDateTime::parse_from_str(&trimmed, "%Y-%m-%dT%H:%M:%S"))
        .map_err(|e| CroniterError::Other(format!("bad datetime {s:?}: {e}")))
}

fn resolve_tz(name: &str) -> Result<Tz, CroniterError> {
    name.parse::<Tz>()
        .map_err(|_| CroniterError::Other(format!("unknown timezone {name:?}")))
}

fn build_options(req: &Request) -> Options {
    Options {
        ret_type: match req.ret.as_deref() {
            Some("datetime") => RetType::DateTime,
            _ => RetType::Timestamp,
        },
        day_or: req.args.day_or,
        max_years_between_matches: req.args.max_years_between_matches,
        is_prev: false,
        hash_id: req.args.hash_id.as_ref().map(|h| h.as_bytes().to_vec()),
        implement_cron_bug: req.args.implement_cron_bug,
        second_at_beginning: req.args.second_at_beginning,
        expand_from_start_time: req.args.expand_from_start_time,
    }
}

fn make_cron(req: &Request) -> Result<Croniter, CroniterError> {
    let opts = build_options(req);
    let start = req.start.as_deref().map(parse_naive).transpose()?;
    match (&req.tz, start) {
        (Some(tz_name), Some(naive)) => {
            let tz = resolve_tz(tz_name)?;
            let aware: DateTime<Tz> = naive
                .and_local_timezone(tz)
                .earliest()
                .ok_or_else(|| CroniterError::Other("start time does not exist in tz".into()))?;
            Croniter::with_options(&req.expr, None, Some(aware), opts)
        }
        _ => Croniter::with_options(&req.expr, start, None, opts),
    }
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
        "validate" => Ok(json!(Croniter::is_valid(
            &req.expr,
            req.args.hash_id.as_ref().map(|h| h.as_bytes()),
            req.args.second_at_beginning,
        ))),

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
            let expanded = croniter::expand::expand(
                &req.expr,
                req.args.hash_id.as_ref().map(|h| h.as_bytes()),
                req.args.second_at_beginning,
                None,
                None,
            )?;
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
