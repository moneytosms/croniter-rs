//! Rust port of [`pallets-eco/croniter`](https://github.com/pallets-eco/croniter).
//!
//! Behaviour-equivalent to the Python original, including its quirks. Where the two
//! genuinely diverge, the divergence is recorded in `DECISIONS.md` at the repo root.
//!
//! Layering mirrors the Python:
//! - [`expand`] parses an expression into an [`Expanded`] (croniter `_expand`)
//! - [`calc`] searches for the next/previous match in naive local time (croniter `_calc`)
//! - [`tz`] re-attaches the timezone and resolves DST (croniter.py:780-819)

// The parser and the search are deliberately shaped like the Python they came from, so
// that the two can be read side by side and a divergence shows up as a structural
// difference. Collapsing croniter's nested guards into `&&` chains would break that
// correspondence for no behavioural gain, so this lint is off crate-wide rather than
// silenced one site at a time.
#![allow(clippy::collapsible_if)]

pub mod calc;
pub mod error;
pub mod expand;
pub mod expr;
pub mod tz;

use chrono::{DateTime, Datelike, Duration, NaiveDate, NaiveDateTime, Utc};
use chrono_tz::Tz;

pub use error::{CroniterError, Result};
pub use expr::{
    DAY_FIELD, Expanded, Expr, HOUR_FIELD, MINUTE_FIELD, MONTH_FIELD, SECOND_FIELD, UNIX_CRON_LEN,
    YEAR_FIELD,
};

use calc::CalcOptions;

/// croniter's `OVERFLOW32B_MODE`. Always false here: Rust has no 32-bit `time_t` ceiling
/// on any target we build for, so the Y2038 degraded path the Python carries
/// (croniter.py:52-58, 394-399) has nothing to guard against.
pub const OVERFLOW32B_MODE: bool = false;

/// What `get_next` and friends hand back. croniter picks this with a `ret_type` argument
/// holding either the `float` or `datetime.datetime` *type object*.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RetType {
    /// Seconds since the Unix epoch, croniter's `ret_type=float`.
    #[default]
    Timestamp,
    /// A datetime, croniter's `ret_type=datetime.datetime`.
    DateTime,
}

/// A scheduled instant, in whichever representation the caller asked for.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Occurrence {
    Timestamp(f64),
    DateTime(DateTime<Tz>),
    /// A timezone-naive datetime, produced when the start time carried no timezone.
    Naive(NaiveDateTime),
}

impl Occurrence {
    pub fn as_timestamp(&self) -> f64 {
        match self {
            Self::Timestamp(t) => *t,
            Self::DateTime(d) => datetime_to_timestamp(*d),
            Self::Naive(d) => naive_to_timestamp(*d),
        }
    }
}

/// croniter's module-level `datetime_to_timestamp` (croniter.py:142-146).
pub fn naive_to_timestamp(d: NaiveDateTime) -> f64 {
    let epoch = NaiveDate::from_ymd_opt(1970, 1, 1)
        .expect("1970-01-01 is a valid date")
        .and_hms_opt(0, 0, 0)
        .expect("midnight is a valid time");
    let delta = d - epoch;
    delta.num_seconds() as f64 + f64::from(delta.subsec_nanos()) / 1e9
}

/// Same, for an aware datetime: Python strips the tzinfo and subtracts the UTC offset,
/// which is exactly the instant's Unix timestamp.
pub fn datetime_to_timestamp(d: DateTime<Tz>) -> f64 {
    d.timestamp() as f64 + f64::from(d.timestamp_subsec_micros()) / 1e6
}

/// Construction-time options, mirroring croniter's keyword arguments.
#[derive(Debug, Clone)]
pub struct Options {
    pub ret_type: RetType,
    /// croniter `day_or`: union day-of-month with day-of-week rather than intersecting.
    pub day_or: bool,
    pub max_years_between_matches: Option<i64>,
    pub is_prev: bool,
    pub hash_id: Option<Vec<u8>>,
    /// Reproduce the vixie/ISC cron bug where DOM and DOW intersect instead of union.
    pub implement_cron_bug: bool,
    /// Treat a 6-field expression as second-first rather than second-last.
    pub second_at_beginning: bool,
    pub expand_from_start_time: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            ret_type: RetType::Timestamp,
            day_or: true,
            max_years_between_matches: None,
            is_prev: false,
            hash_id: None,
            implement_cron_bug: false,
            second_at_beginning: false,
            expand_from_start_time: false,
        }
    }
}

/// The port of croniter's `croniter` class.
#[derive(Debug, Clone)]
pub struct Croniter {
    expanded: Expanded,
    tz: Option<Tz>,
    cur: f64,
    start_time: f64,
    dst_start_time: f64,
    ret_type: RetType,
    day_or: bool,
    implement_cron_bug: bool,
    max_years_between_matches: i64,
    max_years_explicitly_set: bool,
    is_prev: bool,
}

impl Croniter {
    /// croniter's `__init__` with all defaults, starting from a naive local datetime.
    pub fn new(expr: &str, start: NaiveDateTime) -> Result<Self> {
        Self::with_options(expr, Some(start), None, Options::default())
    }

    /// croniter's `__init__` starting from a timezone-aware datetime.
    pub fn new_tz(expr: &str, start: DateTime<Tz>) -> Result<Self> {
        Self::with_options(expr, None, Some(start), Options::default())
    }

    /// Full constructor. Exactly one of `naive_start` / `aware_start` should be given;
    /// if both are `None` the current wall-clock time is used, as croniter does when
    /// `start_time is None` (croniter.py:319-320).
    pub fn with_options(
        expr: &str,
        naive_start: Option<NaiveDateTime>,
        aware_start: Option<DateTime<Tz>>,
        opts: Options,
    ) -> Result<Self> {
        let max_years_explicitly_set = opts.max_years_between_matches.is_some();
        let max_years_between_matches = opts.max_years_between_matches.unwrap_or(50).max(1);

        let (tz, start_ts) = match (naive_start, aware_start) {
            (_, Some(aware)) => (Some(aware.timezone()), datetime_to_timestamp(aware)),
            (Some(naive), None) => (None, naive_to_timestamp(naive)),
            (None, None) => {
                let now = Utc::now();
                (
                    None,
                    now.timestamp() as f64 + f64::from(now.timestamp_subsec_micros()) / 1e6,
                )
            }
        };

        let expanded = expand::expand(
            expr,
            opts.hash_id.as_deref(),
            opts.second_at_beginning,
            if opts.expand_from_start_time {
                Some(start_ts)
            } else {
                None
            },
            if opts.expand_from_start_time {
                tz
            } else {
                None
            },
        )?;

        Ok(Self {
            expanded,
            tz,
            cur: start_ts,
            start_time: start_ts,
            dst_start_time: start_ts,
            ret_type: opts.ret_type,
            day_or: opts.day_or,
            implement_cron_bug: opts.implement_cron_bug,
            max_years_between_matches,
            max_years_explicitly_set,
            is_prev: opts.is_prev,
        })
    }

    /// croniter's `is_valid` classmethod (croniter.py:1363-1373).
    pub fn is_valid(expr: &str, hash_id: Option<&[u8]>, second_at_beginning: bool) -> bool {
        expand::expand(expr, hash_id, second_at_beginning, None, None).is_ok()
    }

    pub fn expanded(&self) -> &Expanded {
        &self.expanded
    }

    pub fn start_time(&self) -> f64 {
        self.start_time
    }

    pub fn dst_start_time(&self) -> f64 {
        self.dst_start_time
    }

    pub fn is_prev(&self) -> bool {
        self.is_prev
    }

    /// Narrow the search window without marking it as caller-supplied.
    ///
    /// croniter keeps the bound and the "was it given explicitly" flag in two separate
    /// attributes (`_max_years_between_matches` / `_max_years_btw_matches_explicitly_set`,
    /// croniter.py:314-317), and the difference is load-bearing: `all_next` *stops* at the
    /// bound when it was explicit and *raises* `CroniterBadDateError` when it was not
    /// (croniter.py:446-450). Passing `max_years_between_matches` to the constructor sets
    /// both; this sets only the bound, which is what assigning the bare attribute does.
    pub fn set_max_years_between_matches(&mut self, years: i64) {
        self.max_years_between_matches = years.max(1);
    }

    /// Split an epoch-seconds float the way `datetime.fromtimestamp` does.
    ///
    /// Python's `datetime` holds whole microseconds and nothing finer, so `fromtimestamp`
    /// rounds to the nearest microsecond. Keeping chrono's full nanosecond resolution here
    /// is not "more precise", it is a different value: a start of `...T00:00:00.000001`
    /// lands on a float that is ~954ns past the second, and the `-1 microsecond` offset at
    /// the top of the prev search (`calc`, mirroring croniter.py:549) then leaves the
    /// cursor 46ns *below* the second instead of exactly on it. `replace(second=0)` drops
    /// it into the previous minute, and `get_prev` answers a whole day early. Round to
    /// microseconds and the two implementations agree.
    fn split_epoch_micros(ts: f64) -> (i64, u32) {
        let secs = ts.div_euclid(1.0) as i64;
        let micros = (ts.rem_euclid(1.0) * 1e6).round() as u32;
        // Rounding up can carry into the next second.
        if micros >= 1_000_000 {
            (secs + 1, 0)
        } else {
            (secs, micros * 1_000)
        }
    }

    /// croniter's `timestamp_to_datetime` (croniter.py:388-402).
    fn timestamp_to_naive(&self, ts: f64) -> NaiveDateTime {
        let (secs, nanos) = Self::split_epoch_micros(ts);
        let utc = DateTime::from_timestamp(secs, nanos)
            .unwrap_or_else(|| DateTime::from_timestamp(0, 0).expect("epoch is representable"));
        match self.tz {
            Some(tz) => utc.with_timezone(&tz).naive_local(),
            None => utc.naive_utc(),
        }
    }

    fn aware_current(&self) -> Option<DateTime<Tz>> {
        let tz = self.tz?;
        let (secs, nanos) = Self::split_epoch_micros(self.cur);
        Some(DateTime::from_timestamp(secs, nanos)?.with_timezone(&tz))
    }

    fn calc_options(&self) -> CalcOptions {
        CalcOptions {
            day_or: self.day_or,
            implement_cron_bug: self.implement_cron_bug,
            max_years_between_matches: self.max_years_between_matches,
        }
    }

    /// croniter's `set_current` (croniter.py:366-377).
    pub fn set_current_naive(&mut self, start: NaiveDateTime) {
        let ts = naive_to_timestamp(start);
        self.tz = None;
        self.start_time = ts;
        self.dst_start_time = ts;
        self.cur = ts;
    }

    pub fn set_current_aware(&mut self, start: DateTime<Tz>) {
        let ts = datetime_to_timestamp(start);
        self.tz = Some(start.timezone());
        self.start_time = ts;
        self.dst_start_time = ts;
        self.cur = ts;
    }

    /// croniter's `get_current` (croniter.py:360-364).
    pub fn get_current(&self, ret_type: Option<RetType>) -> Occurrence {
        match ret_type.unwrap_or(self.ret_type) {
            RetType::Timestamp => Occurrence::Timestamp(self.cur),
            RetType::DateTime => match self.aware_current() {
                Some(d) => Occurrence::DateTime(d),
                None => Occurrence::Naive(self.timestamp_to_naive(self.cur)),
            },
        }
    }

    /// croniter's `get_next` (croniter.py:346-353).
    pub fn get_next(&mut self, ret_type: Option<RetType>) -> Result<Occurrence> {
        self.step(ret_type, false, true)
    }

    /// croniter's `get_prev` (croniter.py:355-358).
    pub fn get_prev(&mut self, ret_type: Option<RetType>) -> Result<Occurrence> {
        self.step(ret_type, true, true)
    }

    fn step(
        &mut self,
        ret_type: Option<RetType>,
        is_prev: bool,
        update_current: bool,
    ) -> Result<Occurrence> {
        self.is_prev = is_prev;
        let ret_type = ret_type.unwrap_or(self.ret_type);

        let result = self.calc_with_tz(is_prev)?;
        let timestamp = result.as_timestamp();
        if update_current {
            self.cur = timestamp;
        }
        Ok(match ret_type {
            RetType::Timestamp => Occurrence::Timestamp(timestamp),
            RetType::DateTime => result,
        })
    }

    /// The naive search plus croniter's DST tail (croniter.py:780-819).
    ///
    /// The Python does this inside `_calc`, which recurses. Here [`calc::calc_next`] is
    /// purely naive and this function drives it re-entrantly, which keeps every timezone
    /// decision in one place.
    fn calc_with_tz(&self, is_prev: bool) -> Result<Occurrence> {
        let now_naive = self.timestamp_to_naive(self.cur);
        let opts = self.calc_options();

        let unaware = calc::calc_next(now_naive, &self.expanded, &opts, is_prev)?;

        let Some(now_aware) = self.aware_current() else {
            // croniter.py:781-782: naive in, naive out.
            return Ok(Occurrence::Naive(unaware));
        };

        // croniter.py:785
        let mut unaware = unaware;
        let mut localized = tz::add_tzinfo(unaware, now_aware, is_prev);

        // croniter.py:787-797. A local time that does not exist is only tolerable when
        // nudging it forward lands somewhere sensible; otherwise keep searching.
        let hour_is_star = self.expanded.fields[HOUR_FIELD].iter().any(|e| e.is_star());
        if !localized.exists
            && (!tz::is_successor(localized.aware, now_aware, is_prev) || hour_is_star)
        {
            let mut guard = 0;
            while !localized.exists {
                unaware = calc::calc_next(unaware, &self.expanded, &opts, is_prev)?;
                localized = tz::add_tzinfo(unaware, now_aware, is_prev);
                guard += 1;
                if guard > 1000 {
                    return Err(CroniterError::BadDate(if is_prev {
                        "failed to find prev date".into()
                    } else {
                        "failed to find next date".into()
                    }));
                }
            }
        }

        // croniter.py:799-802
        let offset_delta = tz::timezone_delta(now_aware, localized.aware);
        if offset_delta == Duration::zero() {
            return Ok(Occurrence::DateTime(localized.aware));
        }

        // croniter.py:804-819. A DST shift means there may be a second candidate at the
        // other UTC offset; take whichever lands nearer to `now`.
        let alternative_start = now_aware.naive_local() + offset_delta;
        let alternative_unaware =
            calc::calc_next(alternative_start, &self.expanded, &opts, is_prev)?;
        let alternative = tz::add_tzinfo(alternative_unaware, now_aware, is_prev);

        if !tz::is_successor(alternative.aware, now_aware, is_prev) {
            return Ok(Occurrence::DateTime(localized.aware));
        }
        if tz::is_successor(localized.aware, alternative.aware, is_prev) {
            return Ok(Occurrence::DateTime(alternative.aware));
        }
        Ok(Occurrence::DateTime(localized.aware))
    }

    /// croniter's `all_next` (croniter.py:431-450). Stops silently on
    /// `CroniterBadDateError` when `max_years_between_matches` was set explicitly,
    /// and propagates it otherwise.
    pub fn all_next(&mut self, ret_type: Option<RetType>, n: usize) -> Result<Vec<Occurrence>> {
        self.collect_many(ret_type, n, false, true)
    }

    /// croniter's `all_prev` (croniter.py:452 onward).
    pub fn all_prev(&mut self, ret_type: Option<RetType>, n: usize) -> Result<Vec<Occurrence>> {
        self.collect_many(ret_type, n, true, true)
    }

    /// `all_next` / `all_prev` with croniter's `update_current` argument.
    ///
    /// With `update_current = false` the cursor never advances, so the generator hands
    /// back the same instant every time rather than walking. That is croniter's documented
    /// behaviour, not a degenerate case: it is how a caller peeks at the next fire time
    /// repeatedly without consuming it.
    pub fn all_from(
        &mut self,
        ret_type: Option<RetType>,
        n: usize,
        is_prev: bool,
        update_current: bool,
    ) -> Result<Vec<Occurrence>> {
        self.collect_many(ret_type, n, is_prev, update_current)
    }

    fn collect_many(
        &mut self,
        ret_type: Option<RetType>,
        n: usize,
        is_prev: bool,
        update_current: bool,
    ) -> Result<Vec<Occurrence>> {
        let mut out = Vec::with_capacity(n);
        for _ in 0..n {
            match self.step(ret_type, is_prev, update_current) {
                Ok(v) => out.push(v),
                Err(e @ CroniterError::BadDate(_)) => {
                    if self.max_years_explicitly_set {
                        break;
                    }
                    return Err(e);
                }
                Err(e) => return Err(e),
            }
        }
        Ok(out)
    }

    /// croniter's `match` classmethod (croniter.py:1376-1386).
    pub fn matches(expr: &str, testdate: NaiveDateTime, day_or: bool) -> Result<bool> {
        Self::match_range(expr, testdate, testdate, day_or)
    }

    /// croniter's `match_range` classmethod (croniter.py:1389-1416).
    pub fn match_range(
        expr: &str,
        from: NaiveDateTime,
        to: NaiveDateTime,
        day_or: bool,
    ) -> Result<bool> {
        let opts = Options {
            ret_type: RetType::DateTime,
            day_or,
            ..Default::default()
        };
        let mut cron = Self::with_options(expr, Some(to), None, opts)?;

        // croniter.py:1405-1408: nudge by a microsecond so an exact hit still counts.
        let mut tdp = cron.timestamp_to_naive(cron.cur);
        if tdp.and_utc().timestamp_subsec_micros() == 0 {
            tdp += Duration::microseconds(1);
        }
        cron.set_current_naive(tdp);

        let tdt = match cron.get_prev(Some(RetType::DateTime)) {
            Ok(v) => v,
            Err(CroniterError::BadDate(_)) => return Ok(false),
            Err(e) => return Err(e),
        };
        let tdt_naive = match tdt {
            Occurrence::Naive(d) => d,
            Occurrence::DateTime(d) => d.naive_local(),
            Occurrence::Timestamp(t) => cron.timestamp_to_naive(t),
        };

        let precision_in_seconds = if cron.expanded.has_seconds() { 1 } else { 60 };
        let duration = (to - from).num_seconds() + precision_in_seconds;
        let gap = (tdp - tdt_naive).num_seconds().abs();
        Ok(gap < duration)
    }
}

/// croniter's module-level `croniter_range` generator (croniter.py:1419-1495).
///
/// Walks forwards when `start < stop` and backwards otherwise. Ends silently on
/// `CroniterBadDateError`, matching the Python's `return` inside the `except`.
pub fn croniter_range(
    start: NaiveDateTime,
    stop: NaiveDateTime,
    expr: &str,
    day_or: bool,
    exclude_ends: bool,
    second_at_beginning: bool,
) -> Result<Vec<NaiveDateTime>> {
    let items = croniter_range_inner(
        start,
        stop,
        None,
        expr,
        day_or,
        exclude_ends,
        second_at_beginning,
    )?;
    Ok(items
        .into_iter()
        .map(|occ| match occ {
            Occurrence::Naive(d) => d,
            Occurrence::DateTime(d) => d.naive_local(),
            Occurrence::Timestamp(t) => DateTime::from_timestamp(t as i64, 0)
                .unwrap_or_default()
                .naive_utc(),
        })
        .collect())
}

/// `croniter_range` over timezone-aware bounds.
///
/// Python does not have a separate entry point for this: `croniter_range` just passes
/// whatever `start` it was handed into the constructor, so an aware `start` yields aware
/// results that fold and unfold across DST. Rust needs the two shapes spelled out because
/// the naive and aware datetimes are different types.
pub fn croniter_range_tz(
    start: DateTime<Tz>,
    stop: DateTime<Tz>,
    expr: &str,
    day_or: bool,
    exclude_ends: bool,
    second_at_beginning: bool,
) -> Result<Vec<DateTime<Tz>>> {
    let tz = start.timezone();
    let items = croniter_range_inner(
        start.naive_local(),
        stop.naive_local(),
        Some(tz),
        expr,
        day_or,
        exclude_ends,
        second_at_beginning,
    )?;
    Ok(items
        .into_iter()
        .map(|occ| match occ {
            Occurrence::DateTime(d) => d,
            other => DateTime::from_timestamp_micros((other.as_timestamp() * 1e6).round() as i64)
                .unwrap_or_default()
                .with_timezone(&tz),
        })
        .collect())
}

fn croniter_range_inner(
    start: NaiveDateTime,
    stop: NaiveDateTime,
    tz: Option<Tz>,
    expr: &str,
    day_or: bool,
    exclude_ends: bool,
    second_at_beginning: bool,
) -> Result<Vec<Occurrence>> {
    let (mut start, mut stop) = (start, stop);
    if !exclude_ends {
        let ms1 = Duration::microseconds(1);
        if start < stop {
            start -= ms1;
            stop += ms1;
        } else {
            start += ms1;
            stop -= ms1;
        }
    }

    let year_span = i64::from((stop.year() - start.year()).abs()) + 1;

    let forward = start < stop;
    let opts = Options {
        ret_type: RetType::DateTime,
        day_or,
        max_years_between_matches: Some(year_span),
        second_at_beginning,
        ..Default::default()
    };

    let localize = |d: NaiveDateTime, what: &str| -> Result<DateTime<Tz>> {
        let tz = tz.expect("only called when a timezone is present");
        d.and_local_timezone(tz)
            .earliest()
            .ok_or_else(|| CroniterError::Other(format!("range {what} not representable in tz")))
    };

    let mut cron = match tz {
        Some(_) => Croniter::with_options(expr, None, Some(localize(start, "start")?), opts)?,
        None => Croniter::with_options(expr, Some(start), None, opts)?,
    };

    // Compare on instants rather than on local wall time. For the naive case the two are
    // the same ordering; across a DST boundary they are not, and Python is comparing
    // aware datetimes, i.e. instants.
    let stop_ts = match tz {
        Some(_) => datetime_to_timestamp(localize(stop, "stop")?),
        None => naive_to_timestamp(stop),
    };

    let mut out = Vec::new();
    loop {
        let next = if forward {
            cron.get_next(Some(RetType::DateTime))
        } else {
            cron.get_prev(Some(RetType::DateTime))
        };
        let occ = match next {
            Ok(o) => o,
            Err(CroniterError::BadDate(_)) => break,
            Err(e) => return Err(e),
        };
        let ts = occ.as_timestamp();
        let keep_going = if forward { ts < stop_ts } else { ts > stop_ts };
        if !keep_going {
            break;
        }
        out.push(occ);
    }
    Ok(out)
}
