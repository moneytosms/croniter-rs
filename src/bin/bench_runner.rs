//! Emits raw per-iteration timings as JSON, for `bench/compare.py`.
//!
//! `cargo bench` (divan, `benches/schedule.rs`) is the interactive tool; this exists
//! because the cross-language comparison needs the *sample distribution* rather than
//! divan's own aggregates. A p99 computed from an ordered sample list is a real p99; one
//! reconstructed from a reported mean and standard deviation is a guess about a
//! distribution that is not normal.
//!
//! Workloads mirror `benches/schedule.rs` and the Python side of `compare.py` exactly.
//! Output shape: `{workload: {expression: [ns, ns, ...]}}`.

use std::collections::BTreeMap;
use std::env;
use std::hint::black_box;
use std::time::Instant;

use chrono::{NaiveDate, NaiveDateTime};
use chrono_tz::America::New_York;
use croniter::{Croniter, RetType, croniter_range};

const EXPRS: &[&str] = &[
    "* * * * *",
    "0 0 * * *",
    "*/5 9-17 * * mon-fri",
    "0 0 1 * *",
    "0 0 L * *",
    "0 0 * * 5#3",
    "0 0 1 1 * 0 2030",
];

const WALK_EXPRS: &[&str] = &["* * * * *", "*/5 9-17 * * mon-fri", "0 0 * * 5#3"];

/// `0 0 1 1 * 0 2030` pins a future year, so a backwards search from the 2026 start has
/// nothing to find. Timing that would measure the give-up path rather than the search,
/// so it is excluded here exactly as it is on the Python side.
const PREV_EXPRS: &[&str] = &[
    "* * * * *",
    "0 0 * * *",
    "*/5 9-17 * * mon-fri",
    "0 0 1 * *",
    "0 0 L * *",
    "0 0 * * 5#3",
];

fn start() -> NaiveDateTime {
    NaiveDate::from_ymd_opt(2026, 3, 8)
        .expect("valid date")
        .and_hms_opt(0, 0, 0)
        .expect("valid time")
}

/// One timed call per sample, so the result is a distribution and not a repeated mean.
fn samples<F: FnMut()>(n: usize, mut f: F) -> Vec<f64> {
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        let t0 = Instant::now();
        f();
        out.push(t0.elapsed().as_nanos() as f64);
    }
    out
}

type Workloads = BTreeMap<String, BTreeMap<String, Vec<f64>>>;

fn main() {
    let n: usize = env::args()
        .nth(1)
        .and_then(|v| v.parse().ok())
        .unwrap_or(2000);

    // `bench_runner 0` is the startup probe: do the smallest real piece of work and
    // exit, so the measured time is process start to first result.
    if n == 0 {
        let mut cron = Croniter::new("* * * * *", start()).expect("expression parses");
        let _ = black_box(cron.get_next(Some(RetType::Timestamp)));
        println!("{{}}");
        return;
    }

    let mut out: Workloads = BTreeMap::new();

    let mut parse = BTreeMap::new();
    for expr in EXPRS {
        parse.insert(
            (*expr).to_string(),
            samples(n, || {
                black_box(Croniter::is_valid(black_box(expr), None, false));
            }),
        );
    }
    out.insert("parse".into(), parse);

    let mut next_once = BTreeMap::new();
    for expr in EXPRS {
        next_once.insert(
            (*expr).to_string(),
            samples(n, || {
                let mut cron = Croniter::new(expr, start()).expect("expression parses");
                black_box(cron.get_next(Some(RetType::Timestamp)).ok());
            }),
        );
    }
    let mut prev_once = BTreeMap::new();
    for expr in PREV_EXPRS {
        prev_once.insert(
            (*expr).to_string(),
            samples(n, || {
                let mut cron = Croniter::new(expr, start()).expect("expression parses");
                black_box(cron.get_prev(Some(RetType::Timestamp)).ok());
            }),
        );
    }
    out.insert("next_once".into(), next_once);
    out.insert("prev_once".into(), prev_once);

    // Parse is outside the timed region on both sides: this is throughput, and folding
    // the parse in would make it look better than it is.
    let walk_n = (n / 40).max(3);
    let mut walk = BTreeMap::new();
    for expr in WALK_EXPRS {
        walk.insert(
            (*expr).to_string(),
            samples(walk_n, || {
                let mut cron = Croniter::new(expr, start()).expect("expression parses");
                for _ in 0..1000 {
                    black_box(cron.get_next(Some(RetType::Timestamp)).ok());
                }
            }),
        );
    }
    out.insert("walk_1000".into(), walk);

    let to = NaiveDate::from_ymd_opt(2027, 3, 8)
        .expect("valid date")
        .and_hms_opt(0, 0, 0)
        .expect("valid time");
    let mut range = BTreeMap::new();
    range.insert(
        "0 0 * * *".to_string(),
        samples((n / 40).max(3), || {
            let _ = black_box(
                croniter_range(start(), to, "0 0 * * *", true, false, false).map(|v| v.len()),
            );
        }),
    );
    out.insert("range_one_year".into(), range);

    let mut dst = BTreeMap::new();
    dst.insert(
        "0 * * * *".to_string(),
        samples((n / 10).max(3), || {
            let aware = start()
                .and_local_timezone(New_York)
                .earliest()
                .expect("start exists");
            let mut cron = Croniter::new_tz("0 * * * *", aware).expect("expression parses");
            for _ in 0..24 {
                black_box(cron.get_next(Some(RetType::DateTime)).ok());
            }
        }),
    );
    out.insert("dst_transition_walk".into(), dst);

    println!(
        "{}",
        serde_json::to_string(&out).expect("timings serialize")
    );
}
