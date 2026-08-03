//! One test per bug that actually shipped, each named for the defect rather than the
//! function.
//!
//! The golden corpus would catch every one of these, but only as an anonymous count
//! changing in a 4 MB JSON blob. These pin the specific behaviour, so a reintroduction
//! fails with a message that says what broke and why it matters.

use chrono::{NaiveDate, NaiveDateTime};
use chrono_tz::America::New_York;
use croniter::{
    Croniter, CroniterError, Occurrence, Options, RetType, croniter_range, croniter_range_tz,
};

fn dt(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> NaiveDateTime {
    NaiveDate::from_ymd_opt(y, mo, d)
        .expect("valid date")
        .and_hms_opt(h, mi, s)
        .expect("valid time")
}

fn dt_micro(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32, micros: u32) -> NaiveDateTime {
    NaiveDate::from_ymd_opt(y, mo, d)
        .expect("valid date")
        .and_hms_micro_opt(h, mi, s, micros)
        .expect("valid time")
}

fn naive_of(occ: Occurrence) -> NaiveDateTime {
    match occ {
        Occurrence::Naive(d) => d,
        Occurrence::DateTime(d) => d.naive_local(),
        Occurrence::Timestamp(t) => chrono::DateTime::from_timestamp(t as i64, 0)
            .expect("timestamp in range")
            .naive_utc(),
    }
}

/// A start one microsecond past midnight must find midnight itself, not the day before.
///
/// The cursor is an `f64` of epoch seconds. Python's `datetime` holds whole microseconds,
/// so `fromtimestamp` rounds; keeping chrono's full nanosecond resolution instead put the
/// cursor ~954ns past the second, and the `-1 microsecond` offset at the top of the prev
/// search (croniter.py:549) then landed 46ns *below* the second. `replace(second=0)`
/// dropped that into the previous minute and the answer came back a whole day early.
#[test]
fn prev_from_one_microsecond_past_a_fire_returns_that_fire() {
    let mut cron =
        Croniter::new("0 0 * * *", dt_micro(2016, 12, 10, 0, 0, 0, 1)).expect("expression parses");
    let got = naive_of(
        cron.get_prev(Some(RetType::DateTime))
            .expect("has a previous"),
    );
    assert_eq!(
        got,
        dt(2016, 12, 10, 0, 0, 0),
        "prev lost the microsecond and skipped back a whole day"
    );
}

/// The same defect seen through `croniter_range`, which is how it reached real callers.
///
/// With `exclude_ends = false` the range nudges its bounds by a microsecond so an exact
/// hit at the boundary still counts. Walking backwards, that nudge is what makes the
/// start instant itself the first result.
#[test]
fn reverse_range_includes_its_start_boundary() {
    let items = croniter_range(
        dt(2016, 12, 10, 0, 0, 0),
        dt(2016, 12, 2, 0, 0, 0),
        "0 0 * * *",
        true,
        false,
        false,
    )
    .expect("range builds");
    assert_eq!(items.first().copied(), Some(dt(2016, 12, 10, 0, 0, 0)));
    assert_eq!(
        items.len(),
        9,
        "expected both boundaries plus the 7 days between"
    );
}

/// A range over timezone-aware bounds must keep the offsets, not flatten to local time.
///
/// 2020-03-08 is US spring-forward, so these four fires straddle a transition: comparing
/// wall-clock times rather than instants gets the boundary wrong.
#[test]
fn range_over_a_dst_transition_keeps_offsets() {
    let start = dt(2020, 3, 7, 0, 0, 0)
        .and_local_timezone(New_York)
        .single()
        .expect("unambiguous");
    let stop = dt(2020, 3, 11, 0, 0, 0)
        .and_local_timezone(New_York)
        .single()
        .expect("unambiguous");
    let items =
        croniter_range_tz(start, stop, "0 3 * * *", true, false, false).expect("range builds");
    let formatted: Vec<String> = items
        .iter()
        .map(|d| d.format("%Y-%m-%dT%H:%M:%S%:z").to_string())
        .collect();
    assert_eq!(
        formatted,
        vec![
            "2020-03-07T03:00:00-05:00",
            "2020-03-08T03:00:00-04:00",
            "2020-03-09T03:00:00-04:00",
            "2020-03-10T03:00:00-04:00",
        ],
        "offset must flip at the transition, and the local hour must stay 03:00"
    );
}

/// `croniter_range` must honour `second_at_beginning`.
///
/// Without it a 6-field expression is read with the extra field as *year* rather than
/// seconds, which silently schedules something entirely different.
#[test]
fn range_honours_second_at_beginning() {
    let items = croniter_range(
        dt(2016, 12, 2, 0, 0, 0),
        dt(2016, 12, 2, 0, 1, 0),
        "*/20 * * * * *",
        true,
        false,
        true,
    )
    .expect("range builds");
    assert_eq!(
        items,
        vec![
            dt(2016, 12, 2, 0, 0, 0),
            dt(2016, 12, 2, 0, 0, 20),
            dt(2016, 12, 2, 0, 0, 40),
            dt(2016, 12, 2, 0, 1, 0),
        ]
    );
}

/// `update_current = false` peeks without consuming: every call answers from the same
/// cursor rather than walking.
#[test]
fn all_from_without_update_current_repeats_one_instant() {
    let mut cron = Croniter::new("* * * * * *", dt(2024, 7, 12, 0, 0, 0)).expect("parses");
    let items = cron
        .all_from(Some(RetType::DateTime), 3, false, false)
        .expect("three steps");
    let got: Vec<NaiveDateTime> = items.into_iter().map(naive_of).collect();
    assert_eq!(
        got,
        vec![dt(2024, 7, 12, 0, 0, 1); 3],
        "the cursor must not advance when update_current is false"
    );

    // And the default still walks, so the flag is doing the work rather than the
    // search having stalled.
    let mut cron = Croniter::new("* * * * * *", dt(2024, 7, 12, 0, 0, 0)).expect("parses");
    let walked: Vec<NaiveDateTime> = cron
        .all_next(Some(RetType::DateTime), 3)
        .expect("three steps")
        .into_iter()
        .map(naive_of)
        .collect();
    assert_eq!(
        walked,
        vec![
            dt(2024, 7, 12, 0, 0, 1),
            dt(2024, 7, 12, 0, 0, 2),
            dt(2024, 7, 12, 0, 0, 3),
        ]
    );
}

/// Bounding the search after construction must still *raise* when it runs out.
///
/// croniter keeps the bound and the "was it given explicitly" flag in two attributes.
/// Passing `max_years_between_matches` to the constructor sets both, and `all_next` then
/// stops quietly at the bound. Assigning the bare attribute sets only the bound, so
/// `all_next` raises instead -- a difference the original suite depends on.
#[test]
fn bound_set_after_construction_still_raises() {
    let expr = "0 13 8 1,4,7,10 wed";
    let start = dt(2020, 9, 24, 0, 0, 0);

    // Explicit via the constructor: stops quietly, yielding nothing.
    let mut explicit = Croniter::with_options(
        expr,
        Some(start),
        None,
        Options {
            day_or: false,
            max_years_between_matches: Some(1),
            ..Default::default()
        },
    )
    .expect("parses");
    assert!(
        explicit
            .all_next(Some(RetType::DateTime), 1)
            .expect("stops rather than erroring")
            .is_empty()
    );

    // Same bound, set afterwards: raises.
    let mut poked = Croniter::with_options(
        expr,
        Some(start),
        None,
        Options {
            day_or: false,
            ..Default::default()
        },
    )
    .expect("parses");
    poked.set_max_years_between_matches(1);
    assert!(matches!(
        poked.all_next(Some(RetType::DateTime), 1),
        Err(CroniterError::BadDate(_))
    ));
}

/// `strict` rejects day/month pairs that can never occur, and consults the year before
/// deciding about February.
#[test]
fn strict_validation_rejects_impossible_dates() {
    let feb31 = croniter::expand::expand("0 0 31 2 *", None, false, None, None).expect("parses");
    assert!(
        croniter::expand::check_strict(&feb31, "0 0 31 2 *", &[]).is_err(),
        "February never has 31 days"
    );

    // Feb 29 is possible in general, so with no year information it must be accepted.
    let feb29 = croniter::expand::expand("0 0 29 2 *", None, false, None, None).expect("parses");
    assert!(croniter::expand::check_strict(&feb29, "0 0 29 2 *", &[]).is_ok());
    assert!(croniter::expand::check_strict(&feb29, "0 0 29 2 *", &[2024]).is_ok());
    assert!(
        croniter::expand::check_strict(&feb29, "0 0 29 2 *", &[2023]).is_err(),
        "2023 is not a leap year, so Feb 29 can never occur"
    );
    assert!(
        croniter::expand::check_strict(&feb29, "0 0 29 2 *", &[2023, 2024]).is_ok(),
        "one leap year in the set is enough"
    );

    // A wildcard day or an L day is always satisfiable and must not be rejected.
    let wild = croniter::expand::expand("0 0 * 2 *", None, false, None, None).expect("parses");
    assert!(croniter::expand::check_strict(&wild, "0 0 * 2 *", &[2023]).is_ok());
}

/// A range must be lazy: taking the first few fires of a decade-long, per-second window
/// has to be instant, not a gigabyte of `Vec`.
///
/// The collecting `croniter_range` on the same bounds would enumerate roughly 300 million
/// instants. Differential fuzzing hit that shape by accident and stalled for 92 seconds,
/// which is what prompted `CroniterRange`. A wall-clock bound is a blunt assertion, but
/// the gap here is nine orders of magnitude, so it is not a flaky one.
#[test]
fn range_iterator_does_not_materialize_the_whole_window() {
    let started = std::time::Instant::now();
    let first: Vec<NaiveDateTime> = croniter::croniter_range_iter(
        dt(2020, 1, 1, 0, 0, 0),
        dt(2030, 1, 1, 0, 0, 0),
        None,
        "* * * * * *",
        true,
        false,
        false,
    )
    .expect("range builds")
    .take(3)
    .map(|occ| naive_of(occ.expect("step succeeds")))
    .collect();

    assert_eq!(
        first,
        vec![
            dt(2020, 1, 1, 0, 0, 0),
            dt(2020, 1, 1, 0, 0, 1),
            dt(2020, 1, 1, 0, 0, 2),
        ]
    );
    assert!(
        started.elapsed().as_secs() < 5,
        "taking 3 items took {:?}; the range is being materialized eagerly",
        started.elapsed()
    );
}

/// Reusing a parse must produce the same schedule as parsing inline.
///
/// This is the property the conformance bridge leans on to give `R` expressions
/// croniter's semantics, and the reason `from_expanded` is public.
#[test]
fn from_expanded_matches_parsing_inline() {
    let expr = "*/5 9-17 * * mon-fri";
    let start = dt(2026, 3, 8, 0, 0, 0);
    let expanded = croniter::expand::expand(expr, None, false, None, None).expect("parses");

    let mut reused = Croniter::from_expanded(expanded, Some(start), None, Options::default());
    let mut inline = Croniter::new(expr, start).expect("parses");

    for _ in 0..50 {
        assert_eq!(
            naive_of(reused.get_next(Some(RetType::DateTime)).expect("advances")),
            naive_of(inline.get_next(Some(RetType::DateTime)).expect("advances")),
        );
    }
}
