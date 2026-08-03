//! Timezone and DST resolution.
//!
//! croniter does its date search in naive local time and only re-attaches the timezone
//! at the very end of `_calc` (croniter.py:780-819). This module is the Rust equivalent
//! of that tail, plus the `_add_tzinfo` / `_is_successor` / `_timezone_delta` helpers
//! (croniter.py:161-233).

use chrono::{DateTime, Duration, NaiveDateTime, Offset, TimeZone, Utc};
use chrono_tz::Tz;

/// Result of re-attaching a timezone to a naive local time.
pub struct Localized {
    pub aware: DateTime<Tz>,
    /// False when the naive time fell in a DST spring-forward gap and had to be nudged
    /// forward to the next time that does exist.
    pub exists: bool,
}

/// Equivalent of croniter's `_add_tzinfo` (croniter.py:179-233), non-pytz branch.
///
/// A naive local time can be missing (spring-forward gap) or doubled (fall-back overlap).
/// Python resolves this with `fold`: `fold=1` when walking backwards, `fold=0` forwards,
/// then checks `datetime_exists` and compares UTC offsets to detect ambiguity.
/// `chrono-tz` hands us the same information up front via `LocalResult`.
pub fn add_tzinfo(naive: NaiveDateTime, previous: DateTime<Tz>, is_prev: bool) -> Localized {
    let tz = previous.timezone();

    match tz.from_local_datetime(&naive) {
        // Unambiguous.
        chrono::LocalResult::Single(aware) => Localized { aware, exists: true },

        // Spring-forward gap: the time does not exist. Python steps forward a minute at a
        // time until it does (croniter.py:220-222). Same loop here, bounded because a DST
        // gap is at most a couple of hours.
        chrono::LocalResult::None => {
            let mut probe = naive;
            for _ in 0..(24 * 60) {
                probe += Duration::minutes(1);
                if let chrono::LocalResult::Single(aware) = tz.from_local_datetime(&probe) {
                    return Localized { aware, exists: false };
                }
                if let chrono::LocalResult::Ambiguous(earliest, _) = tz.from_local_datetime(&probe)
                {
                    return Localized { aware: earliest, exists: false };
                }
            }
            // Unreachable for any real tz database entry; fall back to UTC-equivalent.
            Localized { aware: tz.from_utc_datetime(&naive), exists: false }
        }

        // Fall-back overlap: the time happens twice. Python picks whichever of the two is
        // *closer* to `previous` while still being a successor in the direction of travel
        // (croniter.py:206-215 and 224-233).
        chrono::LocalResult::Ambiguous(earliest, latest) => {
            // Walking backwards, the later (fold=1) instant is the closer one; walking
            // forwards, the earlier one is.
            let (closer, farther) = if is_prev {
                (latest, earliest)
            } else {
                (earliest, latest)
            };
            let aware = if is_successor(closer, previous, is_prev) {
                closer
            } else {
                farther
            };
            Localized { aware, exists: true }
        }
    }
}

/// croniter's `_is_successor` (croniter.py:161-167): strictly after when walking
/// forwards, strictly before when walking backwards. Compared in UTC.
pub fn is_successor(date: DateTime<Tz>, previous: DateTime<Tz>, is_prev: bool) -> bool {
    let a = date.with_timezone(&Utc);
    let b = previous.with_timezone(&Utc);
    if is_prev { a < b } else { a > b }
}

/// croniter's `_timezone_delta` (croniter.py:170-176): how much the UTC offset moved
/// between the two instants. Non-zero means a DST transition was crossed.
pub fn timezone_delta(from: DateTime<Tz>, to: DateTime<Tz>) -> Duration {
    let a = from.offset().fix().local_minus_utc();
    let b = to.offset().fix().local_minus_utc();
    Duration::seconds((b - a) as i64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;
    use chrono_tz::America::New_York;

    fn naive(y: i32, m: u32, d: u32, h: u32, mi: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(y, m, d)
            .unwrap()
            .and_hms_opt(h, mi, 0)
            .unwrap()
    }

    #[test]
    fn ordinary_time_is_unambiguous_and_exists() {
        let prev = New_York.from_local_datetime(&naive(2026, 6, 1, 12, 0)).unwrap();
        let got = add_tzinfo(naive(2026, 6, 1, 13, 0), prev, false);
        assert!(got.exists);
        assert_eq!(got.aware.naive_local(), naive(2026, 6, 1, 13, 0));
    }

    #[test]
    fn spring_forward_gap_does_not_exist_and_is_nudged_forward() {
        // 2026-03-08 02:30 America/New_York never happens: 02:00 jumps to 03:00.
        let prev = New_York.from_local_datetime(&naive(2026, 3, 8, 1, 0)).unwrap();
        let got = add_tzinfo(naive(2026, 3, 8, 2, 30), prev, false);
        assert!(!got.exists, "02:30 on a spring-forward day must not exist");
        assert_eq!(got.aware.naive_local(), naive(2026, 3, 8, 3, 0));
    }

    #[test]
    fn fall_back_overlap_picks_closer_instant_when_moving_forward() {
        // 2026-11-01 01:30 America/New_York happens twice.
        let prev = New_York.from_local_datetime(&naive(2026, 11, 1, 0, 30)).unwrap();
        let got = add_tzinfo(naive(2026, 11, 1, 1, 30), prev, false);
        assert!(got.exists);
        // Moving forward from 00:30 EDT, the first (EDT) occurrence is the closer successor.
        assert_eq!(got.aware.offset().fix().local_minus_utc(), -4 * 3600);
    }

    #[test]
    fn fall_back_overlap_picks_later_instant_when_moving_backward() {
        let prev = New_York
            .from_local_datetime(&naive(2026, 11, 1, 3, 0))
            .unwrap();
        let got = add_tzinfo(naive(2026, 11, 1, 1, 30), prev, true);
        assert!(got.exists);
        // Walking back from 03:00 EST, the second (EST) occurrence is the closer predecessor.
        assert_eq!(got.aware.offset().fix().local_minus_utc(), -5 * 3600);
    }

    #[test]
    fn timezone_delta_is_zero_without_a_transition() {
        let a = New_York.from_local_datetime(&naive(2026, 6, 1, 1, 0)).unwrap();
        let b = New_York.from_local_datetime(&naive(2026, 6, 1, 2, 0)).unwrap();
        assert_eq!(timezone_delta(a, b), Duration::zero());
    }

    #[test]
    fn timezone_delta_is_an_hour_across_spring_forward() {
        let a = New_York.from_local_datetime(&naive(2026, 3, 8, 1, 0)).unwrap();
        let b = New_York.from_local_datetime(&naive(2026, 3, 8, 4, 0)).unwrap();
        assert_eq!(timezone_delta(a, b), Duration::hours(1));
    }

    #[test]
    fn is_successor_respects_direction() {
        let a = New_York.from_local_datetime(&naive(2026, 6, 1, 1, 0)).unwrap();
        let b = New_York.from_local_datetime(&naive(2026, 6, 1, 2, 0)).unwrap();
        assert!(is_successor(b, a, false));
        assert!(!is_successor(a, a, false));
        assert!(is_successor(a, b, true));
    }
}
