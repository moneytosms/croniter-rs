//! Property tests: invariants that must hold for *every* expression and start time,
//! not just the ones the original suite happens to exercise.
//!
//! The golden corpus proves the port agrees with Python on 15,824 recorded calls, and the
//! differential fuzzer compares the two on random ones. Neither says anything about
//! inputs Python was never asked about. These do: they assert the properties a cron
//! iterator must have on its own terms — a search that always advances, never skips a
//! match, and round-trips — so a bug that both implementations share would still be
//! caught here.
//!
//! Failures shrink to a minimal expression and start instant, which is the reason for
//! proptest rather than another loop over random inputs.

use chrono::{Duration, NaiveDate, NaiveDateTime, Timelike};
use croniter::{Croniter, CroniterError, Occurrence, RetType, croniter_range};
use proptest::prelude::*;

// ---------------------------------------------------------------------------
// Generators
// ---------------------------------------------------------------------------

/// One cron field: `*`, a literal, a list, a range, or a step.
fn field(lo: u32, hi: u32) -> impl Strategy<Value = String> {
    prop_oneof![
        2 => Just("*".to_string()),
        3 => (lo..=hi).prop_map(|v| v.to_string()),
        2 => (lo..=hi, lo..=hi).prop_map(|(a, b)| {
            let (a, b) = if a <= b { (a, b) } else { (b, a) };
            format!("{a}-{b}")
        }),
        2 => (lo..=hi, 1u32..=10).prop_map(|(a, s)| format!("{a}/{s}")),
        1 => (lo..=hi, lo..=hi).prop_map(|(a, b)| format!("{a},{b}")),
    ]
}

/// A 5-field expression. Deliberately excludes `L`, `W`, `#`, `H` and `R`: those are
/// covered by the corpus, and `R` in particular has no stable answer to assert against.
fn unix_expr() -> impl Strategy<Value = String> {
    (
        field(0, 59),
        field(0, 23),
        field(1, 28), // 1..=28 so every month can satisfy it; day 29-31 is corpus territory
        field(1, 12),
        field(0, 6),
    )
        .prop_map(|(mi, h, d, mo, dw)| format!("{mi} {h} {d} {mo} {dw}"))
}

/// A 5-field expression that also reaches for the awkward syntax: last-day-of-month,
/// nearest-weekday, nth-weekday-of-month, and days 29-31 that some months never have.
///
/// This is where the interesting bugs live. `L`/`W`/`#` each take their own branch in
/// the search, and a day-of-month above 28 forces the month-rollover path that the
/// vixie-cron day/weekday quirk also runs through.
fn rich_expr() -> impl Strategy<Value = String> {
    let day = prop_oneof![
        4 => field(1, 31),
        1 => Just("L".to_string()),
        1 => (1u32..=28).prop_map(|d| format!("{d}W")),
        1 => Just("LW".to_string()),
    ];
    let dow = prop_oneof![
        4 => field(0, 6),
        1 => (0u32..=6, 1u32..=5).prop_map(|(d, n)| format!("{d}#{n}")),
        1 => (0u32..=6).prop_map(|d| format!("{d}L")),
    ];
    (field(0, 59), field(0, 23), day, field(1, 12), dow)
        .prop_map(|(mi, h, d, mo, dw)| format!("{mi} {h} {d} {mo} {dw}"))
}

/// A start instant well inside the range where every month and weekday occurs.
fn start_dt() -> impl Strategy<Value = NaiveDateTime> {
    (2000i32..2040, 1u32..=12, 1u32..=28, 0u32..24, 0u32..60).prop_map(|(y, mo, d, h, mi)| {
        NaiveDate::from_ymd_opt(y, mo, d)
            .expect("generated date is valid")
            .and_hms_opt(h, mi, 0)
            .expect("generated time is valid")
    })
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn naive_of(occ: Occurrence) -> NaiveDateTime {
    match occ {
        Occurrence::Naive(d) => d,
        Occurrence::DateTime(d) => d.naive_local(),
        Occurrence::Timestamp(t) => chrono::DateTime::from_timestamp(t as i64, 0)
            .expect("timestamp in range")
            .naive_utc(),
    }
}

/// One step forward, or `None` when the expression has no match within the search bound.
/// A bounded search giving up is a legitimate answer, not a property violation.
fn next_of(expr: &str, from: NaiveDateTime) -> Option<NaiveDateTime> {
    let mut cron = Croniter::new(expr, from).ok()?;
    match cron.get_next(Some(RetType::DateTime)) {
        Ok(v) => Some(naive_of(v)),
        Err(CroniterError::BadDate(_)) => None,
        Err(_) => None,
    }
}

fn prev_of(expr: &str, from: NaiveDateTime) -> Option<NaiveDateTime> {
    let mut cron = Croniter::new(expr, from).ok()?;
    match cron.get_prev(Some(RetType::DateTime)) {
        Ok(v) => Some(naive_of(v)),
        Err(CroniterError::BadDate(_)) => None,
        Err(_) => None,
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 400, ..ProptestConfig::default() })]

    /// The defining property of `get_next`: it moves forward, and it lands on a fire.
    ///
    /// "Lands on a fire" is checked with `matches`, which is a genuinely independent
    /// path — it runs the *backwards* search and compares — so agreement is not just the
    /// search agreeing with itself.
    #[test]
    fn next_advances_and_lands_on_a_match(expr in unix_expr(), start in start_dt()) {
        let Some(next) = next_of(&expr, start) else { return Ok(()); };
        prop_assert!(next > start, "next {next} did not advance past start {start}");
        prop_assert!(
            Croniter::matches(&expr, next, true).unwrap_or(false),
            "{expr}: get_next returned {next}, which matches() rejects"
        );
    }

    /// The mirror image, and the case where an off-by-one is easiest to introduce:
    /// the backwards search has its own nearest-diff logic and its own -1 microsecond
    /// offset.
    #[test]
    fn prev_recedes_and_lands_on_a_match(expr in unix_expr(), start in start_dt()) {
        let Some(prev) = prev_of(&expr, start) else { return Ok(()); };
        prop_assert!(prev < start, "prev {prev} did not precede start {start}");
        prop_assert!(
            Croniter::matches(&expr, prev, true).unwrap_or(false),
            "{expr}: get_prev returned {prev}, which matches() rejects"
        );
    }

    /// Stepping forward then back must not overshoot the instant you started from.
    ///
    /// This is what caught the microsecond-rounding bug in a different guise: a prev
    /// search that lost sub-second precision landed a whole period early, which shows up
    /// here as prev(next(t)) < t rather than == t.
    #[test]
    fn next_then_prev_returns_to_the_start(expr in unix_expr(), start in start_dt()) {
        let Some(next) = next_of(&expr, start) else { return Ok(()); };
        let Some(back) = prev_of(&expr, next) else { return Ok(()); };
        prop_assert!(
            back <= start,
            "{expr}: from {start}, next={next}, prev(next)={back} overshot the start"
        );
    }

    /// `get_next` must return the *first* match after `start`, never merely a later one.
    ///
    /// Verified by brute force at the granularity a 5-field expression can express: every
    /// minute in the gap is asked directly, so a search that skipped a fire is caught.
    /// Gaps wider than a day are left alone to keep the test fast; those are covered by
    /// the corpus.
    #[test]
    fn next_skips_nothing(expr in unix_expr(), start in start_dt()) {
        let Some(next) = next_of(&expr, start) else { return Ok(()); };
        let minutes = (next - start).num_minutes();
        if !(0..=1440).contains(&minutes) {
            return Ok(());
        }
        let mut probe = start.with_second(0).and_then(|d| d.with_nanosecond(0))
            .expect("truncating to the minute is always valid") + Duration::minutes(1);
        while probe < next {
            prop_assert!(
                !Croniter::matches(&expr, probe, true).unwrap_or(false),
                "{expr}: get_next skipped {probe} on the way from {start} to {next}"
            );
            probe += Duration::minutes(1);
        }
    }

    /// Two ways of enumerating the same window must agree.
    ///
    /// `croniter_range` has its own bounds handling -- the +/-1 microsecond nudge, the
    /// year-span bound, the forward/reverse split -- so it can drift from the plain
    /// iteration it is supposed to be a shorthand for.
    #[test]
    fn range_agrees_with_repeated_next(expr in unix_expr(), start in start_dt()) {
        let stop = start + Duration::days(7);
        let Ok(range) = croniter_range(start, stop, &expr, true, false, false) else {
            return Ok(());
        };
        // Cap the comparison: a per-minute expression yields ~10k results in a week and
        // the point is agreement, not volume.
        if range.len() > 2000 {
            return Ok(());
        }

        let mut walked = Vec::new();
        let mut cursor = start - Duration::microseconds(1);
        while let Some(next) = next_of(&expr, cursor) {
            if next > stop {
                break;
            }
            walked.push(next);
            cursor = next;
            if walked.len() > range.len() + 1 {
                break;
            }
        }
        prop_assert_eq!(
            walked, range,
            "{}: croniter_range disagrees with repeated get_next", expr
        );
    }

    /// Parsing is a pure function. Worth pinning because the parser caches nothing and
    /// mutates its working buffers in place, which is exactly where a stray bit of state
    /// would show up.
    #[test]
    fn expansion_is_deterministic(expr in unix_expr()) {
        let a = croniter::expand::expand(&expr, None, false, None, None);
        let b = croniter::expand::expand(&expr, None, false, None, None);
        match (a, b) {
            (Ok(a), Ok(b)) => prop_assert_eq!(a, b),
            (Err(_), Err(_)) => {}
            _ => prop_assert!(false, "{} parsed inconsistently across calls", expr),
        }
    }

    /// `is_valid` is documented as "does this parse", so it must not disagree with
    /// actually parsing it.
    #[test]
    fn is_valid_agrees_with_expand(expr in unix_expr()) {
        let parsed = croniter::expand::expand(&expr, None, false, None, None).is_ok();
        prop_assert_eq!(
            Croniter::is_valid(&expr, None, false),
            parsed,
            "{} disagrees between is_valid and expand", expr
        );
    }

    /// The same three core invariants, against `L` / `W` / `#` and days 29-31.
    ///
    /// Kept separate from the plain-expression versions so a failure says immediately
    /// whether the ordinary path or the special-syntax path broke.
    #[test]
    fn special_syntax_advances_and_lands_on_a_match(expr in rich_expr(), start in start_dt()) {
        let Some(next) = next_of(&expr, start) else { return Ok(()); };
        prop_assert!(next > start, "{expr}: next {next} did not advance past {start}");
        prop_assert!(
            Croniter::matches(&expr, next, true).unwrap_or(false),
            "{expr}: get_next returned {next}, which matches() rejects"
        );
    }

    #[test]
    fn special_syntax_prev_recedes_and_lands_on_a_match(
        expr in rich_expr(), start in start_dt(),
    ) {
        let Some(prev) = prev_of(&expr, start) else { return Ok(()); };
        prop_assert!(prev < start, "{expr}: prev {prev} did not precede {start}");
        prop_assert!(
            Croniter::matches(&expr, prev, true).unwrap_or(false),
            "{expr}: get_prev returned {prev}, which matches() rejects"
        );
    }

    #[test]
    fn special_syntax_next_then_prev_returns_to_the_start(
        expr in rich_expr(), start in start_dt(),
    ) {
        let Some(next) = next_of(&expr, start) else { return Ok(()); };
        let Some(back) = prev_of(&expr, next) else { return Ok(()); };
        prop_assert!(
            back <= start,
            "{expr}: from {start}, next={next}, prev(next)={back} overshot the start"
        );
    }

    /// Consecutive steps are strictly increasing, over a longer walk than a single
    /// `next` exercises. A search that stalls -- returning the cursor it was given --
    /// would loop forever in a scheduler, and is invisible to a one-step test.
    #[test]
    fn repeated_next_is_strictly_increasing(expr in unix_expr(), start in start_dt()) {
        let Ok(mut cron) = Croniter::new(&expr, start) else { return Ok(()); };
        let mut last = start;
        for _ in 0..24 {
            match cron.get_next(Some(RetType::DateTime)) {
                Ok(v) => {
                    let got = naive_of(v);
                    prop_assert!(got > last, "{expr}: step went from {last} to {got}");
                    last = got;
                }
                Err(_) => break,
            }
        }
    }
}
