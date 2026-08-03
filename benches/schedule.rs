//! Benchmarks for the port. Methodology is written up in `bench/methodology.md`.
//!
//! Workloads are chosen to mirror what the original test suite actually exercises:
//! parsing, a single step, and a long walk. Run with `cargo bench`.

use chrono::NaiveDate;
use croniter::{Croniter, RetType, croniter_range};

fn main() {
    divan::main();
}

const START: (i32, u32, u32, u32, u32) = (2026, 3, 8, 0, 0);

fn start() -> chrono::NaiveDateTime {
    NaiveDate::from_ymd_opt(START.0, START.1, START.2)
        .expect("valid date")
        .and_hms_opt(START.3, START.4, 0)
        .expect("valid time")
}

/// Expressions spanning the cheap and the expensive ends of the parser.
const EXPRS: &[&str] = &[
    "* * * * *",
    "0 0 * * *",
    "*/5 9-17 * * mon-fri",
    "0 0 1 * *",
    "0 0 L * *",
    "0 0 * * 5#3",
    "0 0 1 1 * 0 2030",
];

#[divan::bench(args = EXPRS)]
fn parse(expr: &str) -> bool {
    Croniter::is_valid(divan::black_box(expr), None, false)
}

#[divan::bench(args = EXPRS)]
fn next_once(bencher: divan::Bencher, expr: &str) {
    bencher
        .with_inputs(|| Croniter::new(expr, start()).expect("expression parses"))
        .bench_local_refs(|cron| cron.get_next(Some(RetType::Timestamp)));
}

#[divan::bench(args = EXPRS)]
fn prev_once(bencher: divan::Bencher, expr: &str) {
    bencher
        .with_inputs(|| Croniter::new(expr, start()).expect("expression parses"))
        .bench_local_refs(|cron| cron.get_prev(Some(RetType::Timestamp)));
}

/// The throughput workload: 1000 consecutive steps from one parse.
#[divan::bench(args = ["* * * * *", "*/5 9-17 * * mon-fri", "0 0 * * 5#3"])]
fn walk_1000(bencher: divan::Bencher, expr: &str) {
    bencher
        .with_inputs(|| Croniter::new(expr, start()).expect("expression parses"))
        .bench_local_refs(|cron| {
            for _ in 0..1000 {
                let _ = cron.get_next(Some(RetType::Timestamp));
            }
        });
}

/// A year of daily fires, the workload most like real scheduler use.
#[divan::bench]
fn range_one_year() -> usize {
    let from = start();
    let to = NaiveDate::from_ymd_opt(2027, 3, 8)
        .expect("valid date")
        .and_hms_opt(0, 0, 0)
        .expect("valid time");
    croniter_range(from, to, "0 0 * * *", true, false)
        .map(|v| v.len())
        .unwrap_or(0)
}

/// Crossing a DST transition, the expensive path in `calc_with_tz`.
#[divan::bench]
fn dst_transition_walk(bencher: divan::Bencher) {
    use chrono_tz::America::New_York;
    bencher
        .with_inputs(|| {
            let naive = NaiveDate::from_ymd_opt(2026, 3, 8)
                .expect("valid date")
                .and_hms_opt(0, 0, 0)
                .expect("valid time");
            let aware = naive
                .and_local_timezone(New_York)
                .earliest()
                .expect("start exists");
            Croniter::new_tz("0 * * * *", aware).expect("expression parses")
        })
        .bench_local_refs(|cron| {
            for _ in 0..24 {
                let _ = cron.get_next(Some(RetType::DateTime));
            }
        });
}
