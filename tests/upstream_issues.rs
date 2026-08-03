//! Behaviour on open bug reports filed against the Python original.
//!
//! These are not tests that the port is *correct*. They are tests that the port is
//! *equivalent*, on inputs where the original is known to be wrong.
//!
//! Three issues were open against `pallets-eco/croniter` at submission time, all filed by
//! other people and each with a reproducer. Each was run against the pinned Python and
//! against this port, and the two agree on all three. That agreement is the point: a port
//! whose goal is behavioural equivalence has to reproduce the original's bugs, not quietly
//! improve on them, or every downstream caller that has already worked around one breaks.
//!
//! Deliberately fixing any of these would be a divergence and would belong in
//! `DECISIONS.md` with a rationale. See `docs/DEFECTS.md` section A.
//!
//! If a future upstream release fixes one of these, the corresponding test here fails,
//! which is the signal to revisit rather than a regression in the port.

use chrono::{NaiveDate, TimeZone};
use chrono_tz::Australia::{Lord_Howe, Sydney};
use croniter::{Croniter, Occurrence, Options, RetType, croniter_range_tz, expand};

/// Upstream issue 259: `croniter_range`'s stop test compares wall-clock readings rather
/// than instants, so a window that spans a fall-back fold silently loses results.
///
/// The window runs from 02:22 +11:00 to 02:26 +10:00, which is 64 minutes as elapsed time
/// and contains six `*/13` fires. Python returns one. This port also returns one, by a
/// different route: it reduces both bounds to naive local time before re-localising, which
/// discards the fold exactly as Python's comparison does.
#[test]
fn issue_259_range_across_a_fold_returns_one_fire_like_python() {
    let start = Sydney
        .with_ymd_and_hms(2019, 4, 7, 2, 22, 0)
        .earliest()
        .expect("02:22 exists on this date");
    let stop = Sydney
        .with_ymd_and_hms(2019, 4, 7, 2, 26, 0)
        .latest()
        .expect("02:26 exists on this date");

    // The bounds really are an hour apart as instants; this is not a degenerate window.
    assert_eq!(stop.timestamp() - start.timestamp(), 3840);

    let got = croniter_range_tz(start, stop, "*/13 * * * *", true, false, false)
        .expect("range is well formed");

    assert_eq!(
        got.len(),
        1,
        "matching Python, which returns 1 here rather than the 6 fires the interval \
         actually contains; if this starts returning 6 the upstream bug was fixed"
    );
}

/// Upstream issue 258: on a zone whose DST shift is 30 minutes rather than a whole hour,
/// `get_next` skips a fire that `get_prev` and `match` both accept.
///
/// Stepping back from the result of `get_next` lands *after* the start that produced it,
/// which cannot be right under any reading. This port reproduces it.
#[test]
fn issue_258_lord_howe_half_hour_shift_overshoots_like_python() {
    let start = Lord_Howe
        .with_ymd_and_hms(2019, 10, 6, 1, 43, 0)
        .earliest()
        .expect("01:43 exists on this date");

    let mut fwd = Croniter::with_options(
        "0 * * * *",
        None,
        Some(start),
        Options {
            ret_type: RetType::DateTime,
            ..Default::default()
        },
    )
    .expect("expression parses");
    let next = match fwd.get_next(Some(RetType::DateTime)).expect("a next fire") {
        Occurrence::DateTime(d) => d,
        other => panic!("expected an aware datetime, got {other:?}"),
    };

    let mut back = Croniter::with_options(
        "0 * * * *",
        None,
        Some(next),
        Options {
            ret_type: RetType::DateTime,
            ..Default::default()
        },
    )
    .expect("expression parses");
    let prev = match back.get_prev(Some(RetType::DateTime)).expect("a prev fire") {
        Occurrence::DateTime(d) => d,
        other => panic!("expected an aware datetime, got {other:?}"),
    };

    // The defect: a backward step from a fire time lands after the start that generated it.
    assert!(
        prev > start,
        "matching Python's overshoot; if prev stops landing after start, upstream changed"
    );
}

/// Upstream issue 252: under `expand_from_start_time`, a two-bound stepped range discards
/// the bounds the expression declares and fires outside them.
///
/// `10-50/15` expands to `[10, 25, 40]` normally. Re-based from a start at minute 7 it
/// becomes `[7, 22, 37]`, and 7 is below the declared lower bound of 10.
#[test]
fn issue_252_expand_from_start_time_drops_declared_bounds_like_python() {
    let plain =
        expand::expand("10-50/15 * * * *", None, false, None, None).expect("expression parses");
    let minutes: Vec<i64> = plain.fields[0].iter().filter_map(|e| e.as_num()).collect();
    assert_eq!(minutes, vec![10, 25, 40], "the declared range, unmodified");

    let start = NaiveDate::from_ymd_opt(2024, 7, 11)
        .expect("valid date")
        .and_hms_opt(10, 7, 0)
        .expect("valid time");
    let rebased = expand::expand(
        "10-50/15 * * * *",
        None,
        false,
        Some(start.and_utc().timestamp() as f64),
        None,
    )
    .expect("expression parses");
    let rebased_minutes: Vec<i64> = rebased.fields[0]
        .iter()
        .filter_map(|e| e.as_num())
        .collect();

    assert_eq!(
        rebased_minutes,
        vec![7, 22, 37],
        "matching Python, where 7 falls below the declared lower bound of 10; if this \
         starts respecting the bound the upstream bug was fixed"
    );
}
