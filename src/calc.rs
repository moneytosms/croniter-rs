//! Port of croniter's date search: `_calc_next`, `_calc`, and the `_get_*_nearest_diff`
//! helpers (croniter.py:476-883). See that file for the reference implementation this
//! is meant to match value-for-value.
//!
//! Scope boundary: everything here works in NAIVE local time. Timezone/DST handling
//! (croniter.py:784-819) is owned by another module and is intentionally not ported.

use crate::error::{CroniterError, Result};
use crate::expr::{
    DAY_FIELD, DOW_FIELD, Expanded, Expr, HOUR_FIELD, MINUTE_FIELD, MONTH_FIELD, NTH_LAST,
    SECOND_FIELD, YEAR_FIELD, last_day_of_month,
};
use chrono::{Datelike, Duration, NaiveDate, NaiveDateTime, Timelike};
use std::collections::{BTreeMap, BTreeSet};

const MONTHS_IN_YEAR: i64 = 12;

pub struct CalcOptions {
    pub day_or: bool,
    pub implement_cron_bug: bool,
    pub max_years_between_matches: i64,
}

fn bad_date_msg(is_prev: bool) -> String {
    if is_prev {
        "failed to find prev date".to_string()
    } else {
        "failed to find next date".to_string()
    }
}

fn bad_type_range(what: &str) -> CroniterError {
    CroniterError::BadTypeRange(format!("date value out of range: {what}"))
}

fn is_all(field: &[Expr]) -> bool {
    // croniter's expanded fields carry `["*"]` (a singleton) to mean "matches everything";
    // `.iter().any` rather than indexing [0] so a malformed field can't panic here.
    field.iter().any(|e| e.is_star())
}

/// Equivalent of croniter._calc_next (croniter.py:476-538).
pub fn calc_next(
    current: NaiveDateTime,
    exp: &Expanded,
    opts: &CalcOptions,
    is_prev: bool,
) -> Result<NaiveDateTime> {
    let fields = exp.fields.clone();
    let nth = exp.nth_weekday_of_month.clone();

    // croniter.py:501-505 - '#' (nth_weekday_of_month) and 'W' (nearest_weekday) are not
    // carried by the DAY/DOW fields themselves, so blanking a field to "*" below does not
    // remove them. We capture the W-day candidates from the *unblanked* DAY_FIELD once,
    // up front, and thread them through both the t1 and t2 sub-calculations exactly like
    // croniter's separately-stored `self.nearest_weekday` set persists across the blanking.
    let w_days: Vec<i64> = if exp.nearest_weekday {
        exp.fields[DAY_FIELD]
            .iter()
            .filter_map(|e| e.as_num())
            .collect()
    } else {
        Vec::new()
    };

    let day_not_star = !is_all(&fields[DAY_FIELD]);
    let dow_not_star = !is_all(&fields[DOW_FIELD]);

    let day_starts_star = exp
        .expressions
        .get(DAY_FIELD)
        .map(|s| s.starts_with('*'))
        .unwrap_or(false);
    let dow_starts_star = exp
        .expressions
        .get(DOW_FIELD)
        .map(|s| s.starts_with('*'))
        .unwrap_or(false);

    let vixie_cron_bug = opts.implement_cron_bug && (day_starts_star || dow_starts_star);

    if day_not_star && dow_not_star && opts.day_or && !vixie_cron_bug {
        // Union (OR) semantics: DOM and DOW are each satisfied independently, then the
        // earlier (or later, if is_prev) of the two results wins.
        let clean_split = exp.nth_weekday_of_month.is_empty() && !exp.nearest_weekday;

        let mut fields_t1 = fields.clone();
        fields_t1[DOW_FIELD] = vec![Expr::Star];
        let mut nth_t1 = nth.clone();
        let t1 = match calc(
            current,
            &fields_t1,
            &mut nth_t1,
            &w_days,
            is_prev,
            opts.max_years_between_matches,
        ) {
            Ok(d) => Some(d),
            Err(e) => {
                if !clean_split {
                    return Err(e);
                }
                None
            }
        };

        let mut fields_t2 = fields.clone();
        fields_t2[DAY_FIELD] = vec![Expr::Star];
        let mut nth_t2 = nth.clone();
        let t2 = match calc(
            current,
            &fields_t2,
            &mut nth_t2,
            &w_days,
            is_prev,
            opts.max_years_between_matches,
        ) {
            Ok(d) => Some(d),
            Err(e) => {
                if !clean_split {
                    return Err(e);
                }
                None
            }
        };

        return match (t1, t2) {
            (None, None) => Err(CroniterError::BadDate(bad_date_msg(is_prev))),
            (None, Some(t2)) => Ok(t2),
            (Some(t1), None) => Ok(t1),
            (Some(t1), Some(t2)) => {
                if is_prev {
                    Ok(if t1 > t2 { t1 } else { t2 })
                } else {
                    Ok(if t1 < t2 { t1 } else { t2 })
                }
            }
        };
    }

    // Either the fields don't call for a split, day_or is off, or the vixie cron bug is
    // being reproduced: fall through to a single intersection (AND) calculation with both
    // fields intact (croniter.py:486-493, 538).
    let mut nth_single = nth;
    calc(
        current,
        &fields,
        &mut nth_single,
        &w_days,
        is_prev,
        opts.max_years_between_matches,
    )
}

/// Set hour/minute/second, keeping the same date. All call sites pass small constant
/// literals (0/23, 0/59, 0/59), so this can never fail.
fn set_time(dt: NaiveDateTime, h: u32, mi: u32, s: u32) -> NaiveDateTime {
    dt.date()
        .and_hms_opt(h, mi, s)
        .expect("h/mi/s are always valid literal time components")
}

fn add_days(dt: NaiveDateTime, days: i64) -> Result<NaiveDateTime> {
    dt.checked_add_signed(Duration::days(days))
        .ok_or_else(|| bad_type_range("day arithmetic overflow"))
}

fn add_duration(dt: NaiveDateTime, d: Duration) -> Result<NaiveDateTime> {
    dt.checked_add_signed(d)
        .ok_or_else(|| bad_type_range("time arithmetic overflow"))
}

/// dateutil's relativedelta wraps months by exactly one 12 (it asserts
/// `1 <= abs(months) <= 12`, see relativedelta.py:369). croniter never feeds it a larger
/// delta, so a single wrap-around (not a full div/mod) is the faithful port.
fn add_months_wrap(year: i64, month: i64, delta: i64) -> (i64, i64) {
    let mut m = month + delta;
    let mut y = year;
    if m > 12 {
        y += 1;
        m -= 12;
    } else if m < 1 {
        y -= 1;
        m += 12;
    }
    (y, m)
}

/// dateutil relativedelta's day-clamping behavior when adding a relative `months=` delta:
/// the day is clamped to `min(days_in_new_month, day)` rather than overflowing into the
/// next month (e.g. Jan 31 + 1 month -> Feb 28, not Mar 3). See relativedelta.py:366-378.
///
/// Note: in croniter's actual call sites (proc_month) the day this clamp produces is
/// always immediately overwritten by an absolute day afterwards, so the clamp itself
/// never surfaces in `calc_next`'s output. It's kept here, faithfully implemented and
/// unit-tested, both for fidelity with the reference and because it is genuinely used by
/// `add_months_wrap`'s caller when computing the *month/year* rollover.
fn add_months_clamped(year: i64, month: i64, day: i64, delta: i64) -> (i64, i64, i64) {
    let (y, m) = add_months_wrap(year, month, delta);
    let last = last_day_of_month(y as i32, m as u32) as i64;
    (y, m, day.min(last))
}

fn ymd_hms(year: i64, month: i64, day: i64, h: u32, mi: u32, s: u32) -> Result<NaiveDateTime> {
    let y32 = i32::try_from(year).map_err(|_| bad_type_range("year"))?;
    let m32 = u32::try_from(month).map_err(|_| bad_type_range("month"))?;
    let d32 = u32::try_from(day).map_err(|_| bad_type_range("day"))?;
    NaiveDate::from_ymd_opt(y32, m32, d32)
        .and_then(|d| d.and_hms_opt(h, mi, s))
        .ok_or_else(|| bad_type_range(&format!("{year}-{month}-{day} {h}:{mi}:{s}")))
}

/// Equivalent of croniter._get_next_nearest_diff (croniter.py:825-847).
fn get_next_nearest_diff(x: i64, to_check: &[Expr], range_val: Option<i64>) -> Option<i64> {
    for e in to_check {
        let dv = match e {
            Expr::Last => match range_val {
                Some(rv) => rv,
                // Only the year field ever calls with range_val=None, and the year field
                // never contains "l". Unreachable in practice; skip rather than panic.
                None => continue,
            },
            Expr::Num(n) => {
                if let Some(rv) = range_val {
                    if *n > rv {
                        continue;
                    }
                }
                *n
            }
            Expr::Star => continue, // callers only invoke this once "*" has been ruled out
        };
        if dv >= x {
            return Some(dv - x);
        }
    }
    let rv = range_val?;
    // croniter.py:847 uses the *raw* to_check[0], not the "l"-substituted value. Since "l"
    // sorts last among expanded values, to_check[0] being "l" would mean it was the sole
    // entry, which always matches in the loop above -- so this is never actually "l".
    let first = match to_check.first() {
        Some(Expr::Num(n)) => *n,
        Some(Expr::Last) => rv,
        _ => 0,
    };
    Some(first - x + rv)
}

/// Equivalent of croniter._get_prev_nearest_diff (croniter.py:849-883).
fn get_prev_nearest_diff(x: i64, to_check: &[Expr], range_val: Option<i64>) -> Option<i64> {
    let mut candidates: Vec<Expr> = to_check.to_vec();
    candidates.reverse();

    for d in &candidates {
        if let Expr::Num(n) = d {
            if *n <= x {
                return Some(n - x);
            }
        }
    }
    if candidates.iter().any(|d| matches!(d, Expr::Last)) {
        return Some(-x);
    }
    let rv = range_val?;

    // By this point no "l" remains among candidates (handled above), so every entry is Num.
    let mut candidate = match candidates.first() {
        Some(Expr::Num(n)) => *n,
        _ => 0,
    };
    for c in &candidates {
        if let Expr::Num(n) = c {
            if *n <= rv {
                candidate = *n;
                break;
            }
        }
    }
    if candidate > rv {
        return Some(-rv);
    }
    Some(candidate - x - rv)
}

fn nearest_diff(x: i64, to_check: &[Expr], range_val: Option<i64>, is_prev: bool) -> Option<i64> {
    if is_prev {
        get_prev_nearest_diff(x, to_check, range_val)
    } else {
        get_next_nearest_diff(x, to_check, range_val)
    }
}

/// croniter._get_nth_weekday_of_month (croniter.py:885-894), reimplemented directly
/// instead of via Python's `calendar.Calendar` machinery: collects every day-of-month in
/// `year`/`month` whose weekday matches `day_of_week` (0=Sunday..6=Saturday, croniter's
/// DOW convention), in ascending order.
fn get_nth_weekday_of_month(year: i64, month: i64, day_of_week: i64) -> Result<Vec<i64>> {
    let y32 = i32::try_from(year).map_err(|_| bad_type_range("year"))?;
    let m32 = u32::try_from(month).map_err(|_| bad_type_range("month"))?;
    let last = last_day_of_month(y32, m32) as i64;
    let mut days = Vec::new();
    for day in 1..=last {
        let date = NaiveDate::from_ymd_opt(y32, m32, day as u32)
            .ok_or_else(|| bad_type_range("nth weekday lookup"))?;
        if date.weekday().num_days_from_sunday() as i64 == day_of_week {
            days.push(day);
        }
    }
    Ok(days)
}

/// croniter._get_nearest_weekday (croniter.py:896-921).
fn get_nearest_weekday(year: i64, month: i64, day: i64) -> Result<i64> {
    let y32 = i32::try_from(year).map_err(|_| bad_type_range("year"))?;
    let m32 = u32::try_from(month).map_err(|_| bad_type_range("month"))?;
    let last = last_day_of_month(y32, m32) as i64;
    let day = day.min(last);
    let date = NaiveDate::from_ymd_opt(y32, m32, day as u32)
        .ok_or_else(|| bad_type_range("nearest weekday lookup"))?;
    let weekday = date.weekday().num_days_from_monday(); // 0=Mon..6=Sun, matches calendar.weekday
    if weekday < 5 {
        return Ok(day);
    }
    if weekday == 5 {
        // Saturday
        return Ok(if day > 1 { day - 1 } else { day + 2 });
    }
    // Sunday
    Ok(if day < last { day + 1 } else { day - 2 })
}

/// Equivalent of croniter._calc (croniter.py:540-823), minus the timezone/DST handling at
/// croniter.py:784-819, which is out of scope here (see module docs).
///
/// `nearest_weekday_days` stands in for croniter's separately-tracked `self.nearest_weekday`
/// set (see the comment in `calc_next`): when non-empty, day-of-month is driven by "W" nearest
/// weekday logic instead of `fields[DAY_FIELD]`/`fields[DOW_FIELD]`.
fn calc(
    now: NaiveDateTime,
    fields: &[Vec<Expr>],
    nth_weekday_of_month: &mut BTreeMap<i64, BTreeSet<i64>>,
    nearest_weekday_days: &[i64],
    is_prev: bool,
    max_years_between_matches: i64,
) -> Result<NaiveDateTime> {
    let has_seconds = fields.len() > crate::expr::UNIX_CRON_LEN;
    let has_year = fields.len() == crate::expr::YEAR_CRON_LEN;

    let offset = if is_prev {
        Duration::microseconds(-1)
    } else if has_seconds {
        Duration::seconds(1)
    } else {
        Duration::minutes(1)
    };
    let mut unaware_time = add_duration(now, offset)?;
    unaware_time = if has_seconds {
        unaware_time
            .with_nanosecond(0)
            .ok_or_else(|| bad_type_range("normalize microsecond"))?
    } else {
        unaware_time
            .with_second(0)
            .and_then(|d| d.with_nanosecond(0))
            .ok_or_else(|| bad_type_range("normalize second"))?
    };

    let mut year = unaware_time.year() as i64;
    let current_year = year;

    loop {
        if (year - current_year).abs() > max_years_between_matches {
            return Err(CroniterError::BadDate(bad_date_msg(is_prev)));
        }

        // proc_year (croniter.py:566-585)
        if has_year && !is_all(&fields[YEAR_FIELD]) {
            let diff_year = nearest_diff(
                unaware_time.year() as i64,
                &fields[YEAR_FIELD],
                None,
                is_prev,
            );
            match diff_year {
                None => return Err(CroniterError::BadDate(bad_date_msg(is_prev))),
                Some(0) => {}
                Some(dy) => {
                    let ny = unaware_time.year() as i64 + dy;
                    unaware_time = if is_prev {
                        ymd_hms(ny, 12, 31, 23, 59, 59)?
                    } else {
                        ymd_hms(ny, 1, 1, 0, 0, 0)?
                    };
                    year = unaware_time.year() as i64;
                    continue;
                }
            }
        }

        // proc_month (croniter.py:587-606)
        if !is_all(&fields[MONTH_FIELD]) {
            let diff_month = nearest_diff(
                unaware_time.month() as i64,
                &fields[MONTH_FIELD],
                Some(MONTHS_IN_YEAR),
                is_prev,
            );
            if let Some(dm) = diff_month {
                if dm != 0 {
                    let (ny, nmo, _clamped_day) = add_months_clamped(
                        unaware_time.year() as i64,
                        unaware_time.month() as i64,
                        unaware_time.day() as i64,
                        dm,
                    );
                    unaware_time = if is_prev {
                        let reset_day = last_day_of_month(ny as i32, nmo as u32) as i64;
                        ymd_hms(ny, nmo, reset_day, 23, 59, 59)?
                    } else {
                        ymd_hms(ny, nmo, 1, 0, 0, 0)?
                    };
                    year = unaware_time.year() as i64;
                    continue;
                }
            }
        }

        // proc_nearest_weekday / proc_day_of_month (croniter.py:608-630, 686-710)
        if !nearest_weekday_days.is_empty() {
            let cur_year = unaware_time.year() as i64;
            let cur_month = unaware_time.month() as i64;
            let d_day = unaware_time.day() as i64;
            let mut candidates = Vec::new();
            for &w_day in nearest_weekday_days {
                let candidate = get_nearest_weekday(cur_year, cur_month, w_day)?;
                if (is_prev && candidate <= d_day) || (!is_prev && d_day <= candidate) {
                    candidates.push(candidate);
                }
            }
            // Walking backwards wants the latest candidate, forwards the earliest.
            // Taking it fallibly folds the "no candidate this month" case into the same
            // binding, so the invariant is in the type rather than in a preceding guard
            // that a later edit could drift away from.
            candidates.sort_unstable();
            let target = if is_prev {
                candidates.last().copied()
            } else {
                candidates.first().copied()
            };
            let Some(target) = target else {
                unaware_time = if is_prev {
                    add_days(set_time(unaware_time, 23, 59, 59), -d_day)?
                } else {
                    let days = last_day_of_month(cur_year as i32, cur_month as u32) as i64;
                    add_days(set_time(unaware_time, 0, 0, 0), days - d_day + 1)?
                };
                year = unaware_time.year() as i64;
                continue;
            };
            let diff_day = target - d_day;
            if diff_day != 0 {
                unaware_time = if is_prev {
                    add_days(set_time(unaware_time, 23, 59, 59), diff_day)?
                } else {
                    add_days(set_time(unaware_time, 0, 0, 0), diff_day)?
                };
                year = unaware_time.year() as i64;
                continue;
            }
        } else if !is_all(&fields[DAY_FIELD]) {
            let cur_year = unaware_time.year() as i64;
            let cur_month = unaware_time.month() as i64;
            let d_day = unaware_time.day() as i64;
            let days = last_day_of_month(cur_year as i32, cur_month as u32) as i64;

            let has_last = fields[DAY_FIELD].iter().any(|e| matches!(e, Expr::Last));
            if !(has_last && days == d_day) {
                let diff_day = if is_prev {
                    let prev_month = (cur_month - 2).rem_euclid(MONTHS_IN_YEAR) + 1;
                    let prev_year = if cur_month == 1 {
                        cur_year - 1
                    } else {
                        cur_year
                    };
                    let days_in_prev_month =
                        last_day_of_month(prev_year as i32, prev_month as u32) as i64;
                    nearest_diff(d_day, &fields[DAY_FIELD], Some(days_in_prev_month), is_prev)
                } else {
                    nearest_diff(d_day, &fields[DAY_FIELD], Some(days), is_prev)
                };
                if let Some(dd) = diff_day {
                    if dd != 0 {
                        unaware_time = if is_prev {
                            add_days(set_time(unaware_time, 23, 59, 59), dd)?
                        } else {
                            add_days(set_time(unaware_time, 0, 0, 0), dd)?
                        };
                        year = unaware_time.year() as i64;
                        continue;
                    }
                }
            }
        }

        // proc_day_of_week_nth / proc_day_of_week (croniter.py:632-684, 645-684)
        if !nth_weekday_of_month.is_empty() {
            let cur_year = unaware_time.year() as i64;
            let cur_month = unaware_time.month() as i64;
            let d_day = unaware_time.day() as i64;
            let mut candidates = Vec::new();
            for (&wday, nths) in nth_weekday_of_month.iter() {
                let c = get_nth_weekday_of_month(cur_year, cur_month, wday)?;
                for &n in nths {
                    let candidate = if n == NTH_LAST {
                        match c.last() {
                            Some(v) => *v,
                            None => continue,
                        }
                    } else if (c.len() as i64) < n {
                        continue;
                    } else {
                        c[(n - 1) as usize]
                    };
                    if (is_prev && candidate <= d_day) || (!is_prev && d_day <= candidate) {
                        candidates.push(candidate);
                    }
                }
            }
            // Walking backwards wants the latest candidate, forwards the earliest.
            // Taking it fallibly folds the "no candidate this month" case into the same
            // binding, so the invariant is in the type rather than in a preceding guard
            // that a later edit could drift away from.
            candidates.sort_unstable();
            let target = if is_prev {
                candidates.last().copied()
            } else {
                candidates.first().copied()
            };
            let Some(target) = target else {
                unaware_time = if is_prev {
                    add_days(set_time(unaware_time, 23, 59, 59), -d_day)?
                } else {
                    let days = last_day_of_month(cur_year as i32, cur_month as u32) as i64;
                    add_days(set_time(unaware_time, 0, 0, 0), days - d_day + 1)?
                };
                year = unaware_time.year() as i64;
                continue;
            };
            let diff_day = target - d_day;
            if diff_day != 0 {
                unaware_time = if is_prev {
                    add_days(set_time(unaware_time, 23, 59, 59), diff_day)?
                } else {
                    add_days(set_time(unaware_time, 0, 0, 0), diff_day)?
                };
                year = unaware_time.year() as i64;
                continue;
            }
        } else if !is_all(&fields[DOW_FIELD]) {
            let dow = unaware_time.weekday().num_days_from_sunday() as i64;
            let diff_dow = nearest_diff(dow, &fields[DOW_FIELD], Some(7), is_prev);
            if let Some(dd) = diff_dow {
                if dd != 0 {
                    unaware_time = if is_prev {
                        add_days(set_time(unaware_time, 23, 59, 59), dd)?
                    } else {
                        add_days(set_time(unaware_time, 0, 0, 0), dd)?
                    };
                    year = unaware_time.year() as i64;
                    continue;
                }
            }
        }

        // proc_hour (croniter.py:712-723)
        if !is_all(&fields[HOUR_FIELD]) {
            let diff_hour = nearest_diff(
                unaware_time.hour() as i64,
                &fields[HOUR_FIELD],
                Some(24),
                is_prev,
            );
            if let Some(dh) = diff_hour {
                if dh != 0 {
                    unaware_time = if is_prev {
                        add_duration(
                            set_time(unaware_time, unaware_time.hour(), 59, 59),
                            Duration::hours(dh),
                        )?
                    } else {
                        add_duration(
                            set_time(unaware_time, unaware_time.hour(), 0, 0),
                            Duration::hours(dh),
                        )?
                    };
                    year = unaware_time.year() as i64;
                    continue;
                }
            }
        }

        // proc_minute (croniter.py:725-736)
        if !is_all(&fields[MINUTE_FIELD]) {
            let diff_min = nearest_diff(
                unaware_time.minute() as i64,
                &fields[MINUTE_FIELD],
                Some(60),
                is_prev,
            );
            if let Some(dmin) = diff_min {
                if dmin != 0 {
                    unaware_time = if is_prev {
                        add_duration(
                            set_time(unaware_time, unaware_time.hour(), unaware_time.minute(), 59),
                            Duration::minutes(dmin),
                        )?
                    } else {
                        add_duration(
                            set_time(unaware_time, unaware_time.hour(), unaware_time.minute(), 0),
                            Duration::minutes(dmin),
                        )?
                    };
                    year = unaware_time.year() as i64;
                    continue;
                }
            }
        }

        // proc_second (croniter.py:738-749)
        if has_seconds {
            if !is_all(&fields[SECOND_FIELD]) {
                let diff_sec = nearest_diff(
                    unaware_time.second() as i64,
                    &fields[SECOND_FIELD],
                    Some(60),
                    is_prev,
                );
                if let Some(ds) = diff_sec {
                    if ds != 0 {
                        unaware_time = add_duration(unaware_time, Duration::seconds(ds))?;
                        year = unaware_time.year() as i64;
                        continue;
                    }
                }
            }
        } else {
            // croniter.py:748 - unconditional, never signals a loop restart.
            unaware_time = set_time(unaware_time, unaware_time.hour(), unaware_time.minute(), 0);
        }

        // Nothing changed: stable. croniter.py:780-782 (naive-only branch).
        unaware_time = unaware_time
            .with_nanosecond(0)
            .ok_or_else(|| bad_type_range("normalize microsecond"))?;
        return Ok(unaware_time);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::expr::Expr::{Last, Num, Star};

    fn dt(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(y, mo, d)
            .unwrap()
            .and_hms_opt(h, mi, s)
            .unwrap()
    }

    fn opts() -> CalcOptions {
        CalcOptions {
            day_or: true,
            implement_cron_bug: false,
            max_years_between_matches: 50,
        }
    }

    fn unix_exp(
        minute: Vec<Expr>,
        hour: Vec<Expr>,
        day: Vec<Expr>,
        month: Vec<Expr>,
        dow: Vec<Expr>,
    ) -> Expanded {
        let exprs = vec![
            if minute == vec![Star] {
                "*".into()
            } else {
                "x".into()
            },
            if hour == vec![Star] {
                "*".into()
            } else {
                "x".into()
            },
            if day == vec![Star] {
                "*".into()
            } else {
                "x".into()
            },
            if month == vec![Star] {
                "*".into()
            } else {
                "x".into()
            },
            if dow == vec![Star] {
                "*".into()
            } else {
                "x".into()
            },
        ];
        Expanded {
            fields: vec![minute, hour, day, month, dow],
            nth_weekday_of_month: BTreeMap::new(),
            expressions: exprs,
            nearest_weekday: false,
        }
    }

    // "* * * * *" every minute
    #[test]
    fn every_minute_next() {
        let exp = unix_exp(vec![Star], vec![Star], vec![Star], vec![Star], vec![Star]);
        let cur = dt(2024, 1, 1, 12, 30, 15);
        let next = calc_next(cur, &exp, &opts(), false).unwrap();
        assert_eq!(next, dt(2024, 1, 1, 12, 31, 0));
    }

    #[test]
    fn every_minute_prev() {
        let exp = unix_exp(vec![Star], vec![Star], vec![Star], vec![Star], vec![Star]);
        let cur = dt(2024, 1, 1, 12, 30, 15);
        let prev = calc_next(cur, &exp, &opts(), true).unwrap();
        assert_eq!(prev, dt(2024, 1, 1, 12, 30, 0));
    }

    // "30 9 * * *" specific hour+minute
    #[test]
    fn specific_hour_minute_next() {
        let exp = unix_exp(
            vec![Num(30)],
            vec![Num(9)],
            vec![Star],
            vec![Star],
            vec![Star],
        );
        let cur = dt(2024, 1, 1, 8, 0, 0);
        let next = calc_next(cur, &exp, &opts(), false).unwrap();
        assert_eq!(next, dt(2024, 1, 1, 9, 30, 0));
    }

    #[test]
    fn specific_hour_minute_rolls_to_next_day() {
        let exp = unix_exp(
            vec![Num(30)],
            vec![Num(9)],
            vec![Star],
            vec![Star],
            vec![Star],
        );
        let cur = dt(2024, 1, 1, 10, 0, 0);
        let next = calc_next(cur, &exp, &opts(), false).unwrap();
        assert_eq!(next, dt(2024, 1, 2, 9, 30, 0));
    }

    // month rollover: "0 0 1 * *" first of month at midnight
    #[test]
    fn month_rollover_next() {
        let exp = unix_exp(
            vec![Num(0)],
            vec![Num(0)],
            vec![Num(1)],
            vec![Star],
            vec![Star],
        );
        let cur = dt(2024, 1, 15, 0, 0, 0);
        let next = calc_next(cur, &exp, &opts(), false).unwrap();
        assert_eq!(next, dt(2024, 2, 1, 0, 0, 0));
    }

    #[test]
    fn month_rollover_prev() {
        let exp = unix_exp(
            vec![Num(0)],
            vec![Num(0)],
            vec![Num(1)],
            vec![Star],
            vec![Star],
        );
        let cur = dt(2024, 2, 15, 0, 0, 0);
        let prev = calc_next(cur, &exp, &opts(), true).unwrap();
        assert_eq!(prev, dt(2024, 2, 1, 0, 0, 0));
    }

    // year rollover: "0 0 1 1 *" (only Jan matches), starting in December
    #[test]
    fn year_rollover_next() {
        let exp = unix_exp(
            vec![Num(0)],
            vec![Num(0)],
            vec![Num(1)],
            vec![Num(1)],
            vec![Star],
        );
        let cur = dt(2024, 12, 15, 0, 0, 0);
        let next = calc_next(cur, &exp, &opts(), false).unwrap();
        assert_eq!(next, dt(2025, 1, 1, 0, 0, 0));
    }

    #[test]
    fn year_rollover_prev() {
        let exp = unix_exp(
            vec![Num(0)],
            vec![Num(0)],
            vec![Num(1)],
            vec![Num(1)],
            vec![Star],
        );
        let cur = dt(2025, 1, 15, 0, 0, 0);
        let prev = calc_next(cur, &exp, &opts(), true).unwrap();
        assert_eq!(prev, dt(2025, 1, 1, 0, 0, 0));
    }

    // day-of-month vs day-of-week union: "0 0 1 * 1" (1st of month OR every Monday)
    #[test]
    fn dom_dow_union_next() {
        let exp = unix_exp(
            vec![Num(0)],
            vec![Num(0)],
            vec![Num(1)],
            vec![Star],
            vec![Num(1)],
        );
        // 2024-01-01 is a Monday, so starting on 1/1 0:00:00, next should be Monday 1/8.
        let cur = dt(2024, 1, 1, 0, 0, 0);
        let next = calc_next(cur, &exp, &opts(), false).unwrap();
        assert_eq!(next, dt(2024, 1, 8, 0, 0, 0));
    }

    #[test]
    fn dom_dow_union_prefers_earlier_of_the_two() {
        // "0 0 15 * 3" - 15th of month OR every Wednesday.
        let exp = unix_exp(
            vec![Num(0)],
            vec![Num(0)],
            vec![Num(15)],
            vec![Star],
            vec![Num(3)],
        );
        // 2024-01-01 is Monday. Next Wednesday is 2024-01-03, well before the 15th.
        let cur = dt(2024, 1, 1, 0, 0, 0);
        let next = calc_next(cur, &exp, &opts(), false).unwrap();
        assert_eq!(next, dt(2024, 1, 3, 0, 0, 0));
    }

    #[test]
    fn dom_dow_intersection_with_cron_bug() {
        // vixie cron bug: DAY_FIELD is non-star and starts with '*': not really possible for
        // a bare day number, so use the DOW field starting with '*' via a real "*/2" form is
        // out of scope for expr construction here; instead exercise the intersection branch
        // directly by disabling day_or, which forces AND semantics like the cron bug would.
        let mut o = opts();
        o.day_or = false;
        // "0 0 1 * 1": with intersection semantics, must be the 1st AND a Monday.
        let exp = unix_exp(
            vec![Num(0)],
            vec![Num(0)],
            vec![Num(1)],
            vec![Star],
            vec![Num(1)],
        );
        let cur = dt(2024, 1, 1, 0, 0, 0);
        let next = calc_next(cur, &exp, &o, false).unwrap();
        // Next month where the 1st falls on Monday: 2024-04-01 is a Monday.
        assert_eq!(next, dt(2024, 4, 1, 0, 0, 0));
    }

    // "L" last day of month
    #[test]
    fn last_day_of_month_next() {
        let exp = unix_exp(
            vec![Num(0)],
            vec![Num(0)],
            vec![Last],
            vec![Star],
            vec![Star],
        );
        let cur = dt(2024, 2, 1, 0, 0, 0);
        let next = calc_next(cur, &exp, &opts(), false).unwrap();
        assert_eq!(next, dt(2024, 2, 29, 0, 0, 0)); // 2024 is a leap year
    }

    #[test]
    fn last_day_of_month_prev() {
        let exp = unix_exp(
            vec![Num(0)],
            vec![Num(0)],
            vec![Last],
            vec![Star],
            vec![Star],
        );
        let cur = dt(2024, 2, 15, 0, 0, 0);
        let prev = calc_next(cur, &exp, &opts(), true).unwrap();
        assert_eq!(prev, dt(2024, 1, 31, 0, 0, 0));
    }

    // leap year Feb 29 explicit
    #[test]
    fn leap_year_feb_29_next() {
        let exp = unix_exp(
            vec![Num(0)],
            vec![Num(0)],
            vec![Num(29)],
            vec![Num(2)],
            vec![Star],
        );
        let cur = dt(2023, 1, 1, 0, 0, 0);
        let next = calc_next(cur, &exp, &opts(), false).unwrap();
        assert_eq!(next, dt(2024, 2, 29, 0, 0, 0));
    }

    // "#" nth weekday: 2nd Tuesday of every month
    #[test]
    fn nth_weekday_of_month_next() {
        let mut nth = BTreeMap::new();
        nth.insert(2i64, BTreeSet::from([2i64])); // Tuesday(2)#2
        let mut exp = unix_exp(
            vec![Num(0)],
            vec![Num(0)],
            vec![Star],
            vec![Star],
            vec![Star],
        );
        exp.nth_weekday_of_month = nth;
        // 2024-01-01 is Monday, so Tuesdays are Jan 2, 9, 16, 23, 30. 2nd Tuesday = Jan 9.
        let cur = dt(2024, 1, 1, 0, 0, 0);
        let next = calc_next(cur, &exp, &opts(), false).unwrap();
        assert_eq!(next, dt(2024, 1, 9, 0, 0, 0));
    }

    #[test]
    fn nth_weekday_of_month_last_prev() {
        let mut nth = BTreeMap::new();
        nth.insert(5i64, BTreeSet::from([NTH_LAST])); // last Friday(5)
        let mut exp = unix_exp(
            vec![Num(0)],
            vec![Num(0)],
            vec![Star],
            vec![Star],
            vec![Star],
        );
        exp.nth_weekday_of_month = nth;
        // Last Friday of January 2024 is Jan 26.
        let cur = dt(2024, 2, 1, 0, 0, 0);
        let prev = calc_next(cur, &exp, &opts(), true).unwrap();
        assert_eq!(prev, dt(2024, 1, 26, 0, 0, 0));
    }

    // "W" nearest weekday
    #[test]
    fn nearest_weekday_next() {
        // day 1 of Jan 2024 is a Monday already, so 1W == Jan 1.
        let mut exp = unix_exp(
            vec![Num(0)],
            vec![Num(0)],
            vec![Num(1)],
            vec![Star],
            vec![Star],
        );
        exp.nearest_weekday = true;
        let cur = dt(2023, 12, 15, 0, 0, 0);
        let next = calc_next(cur, &exp, &opts(), false).unwrap();
        assert_eq!(next, dt(2024, 1, 1, 0, 0, 0));
    }

    #[test]
    fn nearest_weekday_saturday_shifts_back() {
        // Sep 1 2024 is a Sunday -> nearest weekday should be Sep 2 (Monday) per croniter's
        // rule (Sunday not at month end shifts forward to Monday).
        let mut exp = unix_exp(
            vec![Num(0)],
            vec![Num(0)],
            vec![Num(1)],
            vec![Star],
            vec![Star],
        );
        exp.nearest_weekday = true;
        let cur = dt(2024, 8, 15, 0, 0, 0);
        let next = calc_next(cur, &exp, &opts(), false).unwrap();
        assert_eq!(next, dt(2024, 9, 2, 0, 0, 0));
    }

    // max_years bail-out
    #[test]
    fn max_years_bail_out() {
        // Field asks for a year far beyond max_years_between_matches from `current`.
        let mut o = opts();
        o.max_years_between_matches = 2;
        // 7-field expanded value (minute,hour,day,month,dow,second,year) asking for a year
        // far beyond max_years_between_matches from `current`.
        let exp = Expanded {
            fields: vec![
                vec![Num(0)],
                vec![Num(0)],
                vec![Num(1)],
                vec![Num(1)],
                vec![Star],
                vec![Num(0)],
                vec![Num(2099)],
            ],
            nth_weekday_of_month: BTreeMap::new(),
            expressions: vec![
                "0".into(),
                "0".into(),
                "1".into(),
                "1".into(),
                "*".into(),
                "0".into(),
                "2099".into(),
            ],
            nearest_weekday: false,
        };
        let cur = dt(2024, 1, 1, 0, 0, 0);
        let err = calc_next(cur, &exp, &o, false).unwrap_err();
        assert_eq!(err.class_name(), "CroniterBadDateError");
        assert_eq!(format!("{err}"), "failed to find next date");
    }

    #[test]
    fn max_years_bail_out_prev() {
        let mut o = opts();
        o.max_years_between_matches = 1;
        let exp = Expanded {
            fields: vec![
                vec![Num(0)],
                vec![Num(0)],
                vec![Num(1)],
                vec![Num(1)],
                vec![Star],
                vec![Num(0)],
                vec![Num(1970)],
            ],
            nth_weekday_of_month: BTreeMap::new(),
            expressions: vec![
                "0".into(),
                "0".into(),
                "1".into(),
                "1".into(),
                "*".into(),
                "0".into(),
                "1970".into(),
            ],
            nearest_weekday: false,
        };
        let cur = dt(2024, 1, 1, 0, 0, 0);
        let err = calc_next(cur, &exp, &o, true).unwrap_err();
        assert_eq!(format!("{err}"), "failed to find prev date");
    }

    // relativedelta clamping semantics (dateutil quirk): Jan 31 + 1 month -> Feb 28
    #[test]
    fn relativedelta_clamps_month_end() {
        assert_eq!(add_months_clamped(2023, 1, 31, 1), (2023, 2, 28));
        // leap year: Jan 31 + 1 month -> Feb 29
        assert_eq!(add_months_clamped(2024, 1, 31, 1), (2024, 2, 29));
        // no clamp needed
        assert_eq!(add_months_clamped(2024, 1, 15, 1), (2024, 2, 15));
        // wraps year forward
        assert_eq!(add_months_clamped(2024, 12, 31, 1), (2025, 1, 31));
        // wraps year backward (negative delta)
        assert_eq!(add_months_clamped(2024, 1, 31, -1), (2023, 12, 31));
        assert_eq!(add_months_clamped(2024, 3, 31, -1), (2024, 2, 29));
    }

    #[test]
    fn add_months_wrap_basic() {
        assert_eq!(add_months_wrap(2024, 11, 2), (2025, 1));
        assert_eq!(add_months_wrap(2024, 3, -5), (2023, 10));
    }
}

#[cfg(test)]
mod debug_probe {
    use super::*;
    use crate::expr::Expr::{Num, Star};
    use std::collections::BTreeMap;

    #[test]
    fn probe_union() {
        let exprs = vec!["0".into(), "0".into(), "1".into(), "*".into(), "1".into()];
        let exp = Expanded {
            fields: vec![
                vec![Num(0)],
                vec![Num(0)],
                vec![Num(1)],
                vec![Star],
                vec![Num(1)],
            ],
            nth_weekday_of_month: BTreeMap::new(),
            expressions: exprs,
            nearest_weekday: false,
        };
        let o = CalcOptions {
            day_or: true,
            implement_cron_bug: false,
            max_years_between_matches: 50,
        };
        let cur = NaiveDate::from_ymd_opt(2024, 1, 1)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap();

        let fields = exp.fields.clone();
        let mut nth_t1 = exp.nth_weekday_of_month.clone();
        let mut fields_t1 = fields.clone();
        fields_t1[DOW_FIELD] = vec![Star];
        let w_days: Vec<i64> = vec![];
        let t1 = calc(cur, &fields_t1, &mut nth_t1, &w_days, false, 50);
        eprintln!("t1 = {:?}", t1);

        let mut nth_t2 = exp.nth_weekday_of_month.clone();
        let mut fields_t2 = fields.clone();
        fields_t2[DAY_FIELD] = vec![Star];
        let t2 = calc(cur, &fields_t2, &mut nth_t2, &w_days, false, 50);
        eprintln!("t2 = {:?}", t2);

        let full = calc_next(cur, &exp, &o, false);
        eprintln!("full = {:?}", full);
    }
}

#[cfg(test)]
mod debug_probe2 {
    use super::*;
    use crate::expr::Expr::{Num, Star};
    use std::collections::BTreeMap;

    #[test]
    fn probe_dom_list() {
        let exprs = vec![
            "03".into(),
            "03".into(),
            "16,30".into(),
            "*".into(),
            "*".into(),
        ];
        // fields order: minute,hour,day,month,dow
        let exp = Expanded {
            fields: vec![
                vec![Num(0)],
                vec![Num(3)],
                vec![Num(16), Num(30)],
                vec![Star],
                vec![Star],
            ],
            nth_weekday_of_month: BTreeMap::new(),
            expressions: exprs,
            nearest_weekday: false,
        };
        let o = CalcOptions {
            day_or: true,
            implement_cron_bug: false,
            max_years_between_matches: 50,
        };
        let cur = NaiveDate::from_ymd_opt(2013, 3, 1)
            .unwrap()
            .and_hms_opt(12, 17, 34)
            .unwrap();
        let next = calc_next(cur, &exp, &o, false);
        eprintln!("next = {:?}", next);
    }
}
