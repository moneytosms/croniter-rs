//! Port of croniter's expression parser: `croniter._expand`, `croniter.expand`,
//! `croniter._get_low_from_current_date_number`, `HashExpander.{do,_expand_divisor,expand}`,
//! and the small hand-rolled regex/alias tables they lean on.
//!
//! Reference: croniter-python/src/croniter/croniter.py (line numbers cited below refer
//! to that file as it stood when this port was written).

use crate::error::{CroniterError, Result};
use crate::expr::*;
use chrono::{Datelike, TimeZone, Timelike};
use chrono_tz::Tz;
use std::collections::{BTreeSet, HashSet};

// ---------------------------------------------------------------------------
// Alias tables (M_ALPHAS, DOW_ALPHAS, ALPHACONV, LOWMAP) -- croniter.py:64-86,271-285
// ---------------------------------------------------------------------------

fn m_alpha(k: &str) -> Option<i64> {
    Some(match k {
        "jan" => 1,
        "feb" => 2,
        "mar" => 3,
        "apr" => 4,
        "may" => 5,
        "jun" => 6,
        "jul" => 7,
        "aug" => 8,
        "sep" => 9,
        "oct" => 10,
        "nov" => 11,
        "dec" => 12,
        _ => return None,
    })
}

fn dow_alpha(k: &str) -> Option<i64> {
    Some(match k {
        "sun" => 0,
        "mon" => 1,
        "tue" => 2,
        "wed" => 3,
        "thu" => 4,
        "fri" => 5,
        "sat" => 6,
        _ => return None,
    })
}

fn is_weekday_name(s: &str) -> bool {
    dow_alpha(s).is_some()
}

fn is_month_name(s: &str) -> bool {
    m_alpha(s).is_some()
}

/// `croniter._alphaconv` (croniter.py:340-344), specialized to what `ALPHACONV` actually
/// holds: `{}` for min/hour/second/year, `{"l": "l"}` for day, `M_ALPHAS` for month,
/// `DOW_ALPHAS` for dow. Returns the decimal-string form of the resolved value (matching
/// how the caller immediately re-stringifies whatever `ALPHACONV` produced).
fn alphaconv(field_index: usize, key: &str, expressions: &[String]) -> Result<String> {
    let resolved = match field_index {
        DAY_FIELD if key == "l" => Some("l".to_string()),
        MONTH_FIELD => m_alpha(key).map(|v| v.to_string()),
        DOW_FIELD => dow_alpha(key).map(|v| v.to_string()),
        _ => None,
    };
    resolved.ok_or_else(|| {
        CroniterError::NotAlpha(format!("[{}] is not acceptable", expressions.join(" ")))
    })
}

/// `croniter.value_alias` (croniter.py:923-939). `LOWMAP` is `({}, {}, {0:1}, {0:1}, {7:0}, {}, {})`.
fn value_alias(val: i64, field_index: usize, len_expressions: usize) -> i64 {
    let lowmap_entry = match field_index {
        DAY_FIELD => Some((0i64, 1i64)),
        MONTH_FIELD => Some((0, 1)),
        DOW_FIELD => Some((7, 0)),
        _ => None,
    };
    if let Some((from, to)) = lowmap_entry {
        if val == from {
            let skip = (matches!(field_index, DAY_FIELD | MONTH_FIELD)
                && len_expressions == UNIX_CRON_LEN)
                || (matches!(field_index, MONTH_FIELD | DOW_FIELD)
                    && len_expressions == SECOND_CRON_LEN)
                || (matches!(field_index, DAY_FIELD | MONTH_FIELD | DOW_FIELD)
                    && len_expressions == YEAR_CRON_LEN);
            if !skip {
                return to;
            }
        }
    }
    val
}

// ---------------------------------------------------------------------------
// Hand-written stand-ins for the module-level regexes. No regex crate is
// available (Cargo.toml is not ours to edit), and every pattern used here is
// small enough to parse by hand.
// ---------------------------------------------------------------------------

fn only_int(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

fn star_or_int(s: &str) -> bool {
    s == "*" || only_int(s)
}

/// `step_search_re = re.compile(r"^([^-]+)-([^-/]+)(/(\d+))?$")`
fn step_search(t: &str) -> Option<(String, String, Option<String>)> {
    let dash = t.find('-')?;
    if dash == 0 {
        return None;
    }
    let low = &t[..dash];
    let rest = &t[dash + 1..];
    if rest.is_empty() || rest.contains('-') {
        return None;
    }
    if let Some(slash) = rest.find('/') {
        let high = &rest[..slash];
        let step = &rest[slash + 1..];
        if high.is_empty() || step.is_empty() || !step.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        Some((low.to_string(), high.to_string(), Some(step.to_string())))
    } else {
        Some((low.to_string(), rest.to_string(), None))
    }
}

/// `nearest_weekday_re = re.compile(r"^(?:(\d+)w|w(\d+))$")`
fn nearest_weekday_match(s: &str) -> Option<i64> {
    if let Some(stripped) = s.strip_suffix('w') {
        if !stripped.is_empty() && stripped.bytes().all(|b| b.is_ascii_digit()) {
            return stripped.parse().ok();
        }
        return None;
    }
    if let Some(stripped) = s.strip_prefix('w') {
        if !stripped.is_empty() && stripped.bytes().all(|b| b.is_ascii_digit()) {
            return stripped.parse().ok();
        }
    }
    None
}

enum DowSpecial {
    /// `(he)#(last)`: nth weekday-of-month syntax, e.g. `mon#2`, `mon-fri#2`, `5#3`.
    Nth { he: String, nth_str: String },
    /// `l(digits)`: last such weekday of the month, e.g. `l3`.
    Last { n_str: String },
}

fn he_valid(he: &str) -> bool {
    if let Some(dash) = he.find('-') {
        let (a, b) = (&he[..dash], &he[dash + 1..]);
        if is_weekday_name(a) && is_weekday_name(b) {
            return true;
        }
        if is_month_name(a) && is_month_name(b) {
            return true;
        }
        // The `\w+` fallback in the Python alternation does not allow '-'.
        false
    } else {
        !he.is_empty() && he.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
    }
}

/// `special_dow_re`, croniter.py:115-118. Fiddliest of the bunch: two big
/// alternatives, `(he)#(digits)` or literal `l(digits)`.
fn special_dow_match(s: &str) -> Option<DowSpecial> {
    if let Some(hash_pos) = s.find('#') {
        let he = &s[..hash_pos];
        let last = &s[hash_pos + 1..];
        if he.is_empty() || last.is_empty() || !last.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        if !he_valid(he) {
            return None;
        }
        return Some(DowSpecial::Nth {
            he: he.to_string(),
            nth_str: last.to_string(),
        });
    }
    if let Some(rest) = s.strip_prefix('l') {
        if !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit()) {
            return Some(DowSpecial::Last {
                n_str: rest.to_string(),
            });
        }
    }
    None
}

struct HashMatch {
    hash_type: char,
    range_begin: Option<i64>,
    range_end: Option<i64>,
    divisor: Option<i64>,
}

/// `hash_expression_re`, croniter.py:121-123.
fn match_hash_expr(expr: &str) -> Option<HashMatch> {
    let bytes = expr.as_bytes();
    if bytes.is_empty() {
        return None;
    }
    let hash_type = match bytes[0] {
        b'h' => 'h',
        b'r' => 'r',
        _ => return None,
    };
    let mut rest = &expr[1..];
    let mut range_begin = None;
    let mut range_end = None;
    if let Some(after_paren) = rest.strip_prefix('(') {
        let close = after_paren.find(')')?;
        let inner = &after_paren[..close];
        let dash = inner.find('-')?;
        let (b, e) = (&inner[..dash], &inner[dash + 1..]);
        if b.is_empty() || e.is_empty() || !only_int(b) || !only_int(e) {
            return None;
        }
        range_begin = Some(b.parse::<i64>().ok()?);
        range_end = Some(e.parse::<i64>().ok()?);
        rest = &after_paren[close + 1..];
    }
    let mut divisor = None;
    if let Some(after_slash) = rest.strip_prefix('/') {
        if !only_int(after_slash) {
            return None;
        }
        divisor = Some(after_slash.parse::<i64>().ok()?);
        rest = "";
    }
    if !rest.is_empty() {
        return None;
    }
    Some(HashMatch {
        hash_type,
        range_begin,
        range_end,
        divisor,
    })
}

// ---------------------------------------------------------------------------
// HashExpander: croniter.py:1498-1585
// ---------------------------------------------------------------------------

fn crc32(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &byte in data {
        crc ^= byte as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

/// `random.randint(0, 0xFFFFFFFF)` has no faithful Rust equivalent without a `rand`
/// dependency (not in Cargo.toml, ours not to add). `h(...)` is deterministic and
/// covered above; `r` is documented by croniter itself as not reproducible, so a
/// process-local pseudo-random source is an acceptable substitute here.
fn random_u32() -> u32 {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let mut hasher = RandomState::new().build_hasher();
    hasher.write_u128(nanos);
    (hasher.finish() & 0xFFFF_FFFF) as u32
}

/// `HashExpander.do`, croniter.py:1502-1512.
fn hash_do(
    idx: usize,
    hash_type: char,
    hash_id: Option<&[u8]>,
    range_begin: i64,
    range_end: i64,
) -> Result<i64> {
    let crc: u32 = if hash_type == 'r' {
        random_u32()
    } else {
        let id = hash_id.ok_or_else(|| {
            CroniterError::BadCron("Hashed definitions must include hash_id".to_string())
        })?;
        crc32(id)
    };
    Ok((((crc as i64) >> idx) % (range_end - range_begin + 1)) + range_begin)
}

/// `HashExpander._expand_divisor`, croniter.py:1517-1537.
fn expand_divisor(
    idx: usize,
    hash_type: char,
    divisor: i64,
    hash_id: Option<&[u8]>,
    range_begin: i64,
    range_end: i64,
) -> Result<String> {
    let x = hash_do(
        idx,
        hash_type,
        hash_id,
        range_begin,
        (range_begin + divisor - 1).min(range_end),
    )?;
    if x == range_end {
        Ok(x.to_string())
    } else {
        Ok(format!("{x}-{range_end}/{divisor}"))
    }
}

/// `HashExpander.expand`, croniter.py:1539-1583.
fn hash_expand(idx: usize, expr: &str, hash_id: Option<&[u8]>) -> Result<String> {
    let Some(m) = match_hash_expr(expr) else {
        return Ok(expr.to_string());
    };
    if m.hash_type == 'h' && hash_id.is_none() {
        return Err(CroniterError::BadCron(
            "Hashed definitions must include hash_id".to_string(),
        ));
    }
    if let (Some(rb), Some(re)) = (m.range_begin, m.range_end) {
        if rb >= re {
            return Err(CroniterError::BadCron(
                "Range end must be greater than range begin".to_string(),
            ));
        }
    }
    match (m.range_begin, m.range_end, m.divisor) {
        (Some(rb), Some(re), Some(div)) => {
            if div == 0 {
                return Err(CroniterError::BadCron(format!("Bad expression: {expr}")));
            }
            expand_divisor(idx, m.hash_type, div, hash_id, rb, re)
        }
        (Some(rb), Some(re), None) => Ok(hash_do(idx, m.hash_type, hash_id, rb, re)?.to_string()),
        (None, None, Some(div)) => {
            if div == 0 {
                return Err(CroniterError::BadCron(format!("Bad expression: {expr}")));
            }
            let (rb, re) = RANGES[idx];
            expand_divisor(idx, m.hash_type, div, hash_id, rb, re)
        }
        _ => {
            let (rb, re) = RANGES[idx];
            Ok(hash_do(idx, m.hash_type, hash_id, rb, re)?.to_string())
        }
    }
}

// ---------------------------------------------------------------------------
// `@` aliases -- croniter.py:949-957
// ---------------------------------------------------------------------------

fn alias_expr(efl: &str, has_hash: bool) -> Option<String> {
    let idx = usize::from(has_hash);
    let pair = match efl {
        "@midnight" => ["0 0 * * *", "h h(0-2) * * * h"],
        "@hourly" => ["0 * * * *", "h * * * * h"],
        "@daily" => ["0 0 * * *", "h h * * * h"],
        "@weekly" => ["0 0 * * 0", "h h * * h h"],
        "@monthly" => ["0 0 1 * *", "h h h * * h"],
        "@yearly" => ["0 0 1 1 *", "h h h h * h"],
        "@annually" => ["0 0 1 1 *", "h h h h * h"],
        _ => return None,
    };
    Some(pair[idx].to_string())
}

// ---------------------------------------------------------------------------
// `croniter._get_low_from_current_date_number`, croniter.py:1336-1360.
// ---------------------------------------------------------------------------

fn get_low_from_current_date_number(
    field_index: usize,
    step: i64,
    from_timestamp: f64,
    tz: Option<Tz>,
) -> Result<i64> {
    let tz = tz.unwrap_or(chrono_tz::UTC);
    let secs = from_timestamp.floor() as i64;
    let nsecs = ((from_timestamp - from_timestamp.floor()) * 1_000_000_000.0)
        .round()
        .clamp(0.0, 999_999_999.0) as u32;
    let dt = tz
        .timestamp_opt(secs, nsecs)
        .single()
        .ok_or_else(|| CroniterError::Other(format!("invalid from_timestamp {from_timestamp}")))?;
    Ok(match field_index {
        MINUTE_FIELD => dt.minute() as i64 % step,
        HOUR_FIELD => dt.hour() as i64 % step,
        DAY_FIELD => ((dt.day() as i64 - 1) % step) + 1,
        MONTH_FIELD => ((dt.month() as i64 - 1) % step) + 1,
        DOW_FIELD => (dt.weekday().number_from_monday() as i64 % 7) % step,
        SECOND_FIELD => dt.second() as i64 % step,
        YEAR_FIELD => {
            let year_start = RANGES[YEAR_FIELD].0;
            ((dt.year() as i64 - year_start) % step) + year_start
        }
        _ => {
            return Err(CroniterError::Other(format!(
                "Can't get current date number for field index {field_index}"
            )));
        }
    })
}

// ---------------------------------------------------------------------------
// Small helpers used while assembling a field's expansion.
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
enum NthKind {
    Num(i64),
    Last,
}

fn num_range_step(a: i64, b: i64, step: i64) -> Vec<i64> {
    if a > b || step <= 0 {
        return Vec::new();
    }
    let mut v = Vec::new();
    let mut x = a;
    while x <= b {
        v.push(x);
        x += step;
    }
    v
}

/// Mirrors `sorted(res, key=lambda i: f"{i:02}" if isinstance(i, int) else i)`, croniter.py:1208.
fn sort_key(e: &Expr) -> String {
    match e {
        Expr::Star => "*".to_string(),
        Expr::Last => "l".to_string(),
        Expr::Num(n) => format!("{n:02}"),
    }
}

fn format_int_set(s: &BTreeSet<i64>) -> String {
    let items: Vec<String> = s.iter().map(|v| v.to_string()).collect();
    format!("{{{}}}", items.join(", "))
}

fn format_nth_map(m: &std::collections::BTreeMap<i64, BTreeSet<i64>>) -> String {
    let items: Vec<String> = m
        .iter()
        .map(|(k, v)| format!("{k}: {}", format_int_set(v)))
        .collect();
    format!("{{{}}}", items.join(", "))
}

// ---------------------------------------------------------------------------
// The parser itself: `croniter._expand` + `croniter.expand`, croniter.py:944-1336.
// ---------------------------------------------------------------------------

/// croniter's `strict` / `strict_year` cross-validation (croniter.py:1233-1266).
///
/// Rejects day/month combinations that can never occur, such as `0 0 31 2 *` (February
/// 31st). February is treated as having 29 days unless the years in play are known and
/// none of them is a leap year, either from `strict_year` or from an explicit year
/// field. Kept out of [`expand`] because it is an opt-in extra check on an
/// already-parsed expression, exactly as it is in the Python.
pub fn check_strict(expanded: &Expanded, expr_format: &str, strict_year: &[i64]) -> Result<()> {
    let days = &expanded.fields[crate::expr::DAY_FIELD];
    let months = &expanded.fields[crate::expr::MONTH_FIELD];
    if days.as_slice() == [Expr::Star]
        || days.as_slice() == [Expr::Last]
        || months.as_slice() == [Expr::Star]
    {
        return Ok(());
    }

    let int_days: Vec<i64> = days.iter().filter_map(|d| d.as_num()).collect();
    let int_months: Vec<i64> = months.iter().filter_map(|m| m.as_num()).collect();
    if int_days.is_empty() || int_months.is_empty() {
        return Ok(());
    }

    let mut days_in_month = crate::expr::DAYS.map(i64::from);
    if int_months.contains(&2) {
        // "Might be a leap year" is the default; only a known, entirely non-leap set of
        // years narrows February to 28.
        let has_leap_year = if !strict_year.is_empty() {
            strict_year.iter().any(|&y| crate::expr::is_leap(y as i32))
        } else if expanded.has_year() {
            let years = &expanded.fields[crate::expr::YEAR_FIELD];
            let int_years: Vec<i64> = years.iter().filter_map(|y| y.as_num()).collect();
            years.as_slice() == [Expr::Star]
                || int_years.is_empty()
                || int_years.iter().any(|&y| crate::expr::is_leap(y as i32))
        } else {
            true
        };
        if has_leap_year {
            days_in_month[1] = 29;
        }
    }

    let min_day = int_days.iter().copied().min().unwrap_or(1);
    let max_possible = int_months
        .iter()
        .filter_map(|&m| {
            usize::try_from(m - 1)
                .ok()
                .and_then(|i| days_in_month.get(i))
        })
        .copied()
        .max()
        .unwrap_or(31);

    if min_day > max_possible {
        return Err(CroniterError::BadCron(format!(
            "[{expr_format}] is not acceptable. Day(s) {int_days:?} \
             can never occur in month(s) {int_months:?}"
        )));
    }
    Ok(())
}

/// Expand a cron expression into croniter's normalized field-list form.
///
/// Faithful port of `croniter._expand`/`croniter.expand` (classmethods). The optional
/// `strict` cross-validation lives in [`check_strict`], mirroring the Python, where it
/// is a separate opt-in branch rather than part of the parse.
pub fn expand(
    expr_format: &str,
    hash_id: Option<&[u8]>,
    second_at_beginning: bool,
    from_timestamp: Option<f64>,
    from_timestamp_tz: Option<Tz>,
) -> Result<Expanded> {
    let efl_raw = expr_format.to_lowercase();
    let efl = alias_expr(&efl_raw, hash_id.is_some()).unwrap_or(efl_raw);

    let mut expressions: Vec<String> = efl.split_whitespace().map(|s| s.to_string()).collect();

    if !(5..=7).contains(&expressions.len()) {
        return Err(CroniterError::BadCron(
            "Exactly 5, 6 or 7 columns has to be specified for iterator expression.".to_string(),
        ));
    }

    if expressions.len() > UNIX_CRON_LEN && second_at_beginning {
        let first = expressions.remove(0);
        expressions.insert(SECOND_FIELD, first);
    }

    let mut expanded: Vec<Vec<Expr>> = Vec::with_capacity(expressions.len());
    let mut nth_weekday_of_month: std::collections::BTreeMap<i64, BTreeSet<i64>> =
        std::collections::BTreeMap::new();
    let mut nearest_weekday_flag = false;

    for field_index in 0..expressions.len() {
        let mut expr = hash_expand(field_index, &expressions[field_index], hash_id)?;

        if expr.contains('?') {
            if expr != "?" {
                return Err(CroniterError::BadCron(format!(
                    "[{expr_format}] is not acceptable. Question mark can not used with other characters"
                )));
            }
            if field_index != DAY_FIELD && field_index != DOW_FIELD {
                return Err(CroniterError::BadCron(format!(
                    "[{expr_format}] is not acceptable. Question mark can only used in day_of_month or day_of_week"
                )));
            }
            expr = "*".to_string();
        }

        let (min_r, max_r) = RANGES[field_index];
        let mut e_list: Vec<String> = expr.split(',').map(|s| s.to_string()).collect();
        let mut res: Vec<Expr> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();

        while let Some(mut e) = e_list.pop() {
            let mut nth: Option<NthKind> = None;

            if field_index == DOW_FIELD {
                if let Some(sd) = special_dow_match(&e) {
                    match sd {
                        DowSpecial::Nth { he, nth_str } => {
                            // Parse and range-check in one step, so the accepted value is
                            // carried by the `Some` arm instead of being re-extracted
                            // after a separate validity flag.
                            let parsed: Option<i64> = nth_str.parse().ok();
                            let Some(nth_val) = parsed.filter(|v| (1..=5).contains(v)) else {
                                // croniter reports the parsed number when the text was
                                // numeric at all, and the raw text otherwise.
                                let shown = parsed
                                    .map(|v| v.to_string())
                                    .unwrap_or_else(|| nth_str.clone());
                                return Err(CroniterError::BadCron(format!(
                                    "[{expr_format}] is not acceptable. Invalid day_of_week value: '{shown}'"
                                )));
                            };
                            e = he;
                            nth = Some(NthKind::Num(nth_val));
                        }
                        DowSpecial::Last { n_str } => {
                            e = n_str;
                            nth = Some(NthKind::Last);
                        }
                    }
                }
            }

            if field_index == DAY_FIELD {
                if let Some(w_day) = nearest_weekday_match(&e) {
                    if !(1..=31).contains(&w_day) {
                        return Err(CroniterError::BadCron(format!(
                            "[{expr_format}] is not acceptable, nearest weekday day value '{w_day}' out of range"
                        )));
                    }
                    if !e_list.is_empty() || !res.is_empty() {
                        return Err(CroniterError::BadCron(format!(
                            "[{expr_format}] is not acceptable. 'W' can only be used with a single day value, not in a list or range"
                        )));
                    }
                    nearest_weekday_flag = true;
                    res.push(Expr::Num(w_day));
                    continue;
                }
            }

            // Normalize "*/step" -> "{min}-{max}/step", then (if that didn't parse as a
            // range) "{start}/{step}" -> "{start}-{max}/{step}". croniter.py:1049-1070.
            let mut t = e.clone();
            if let Some(rest) = e.strip_prefix("*/") {
                if !rest.is_empty() {
                    t = format!("{min_r}-{max_r}/{rest}");
                }
            }
            let mut m = step_search(&t);
            let mut start_with_step = false;
            if m.is_none() {
                t = e.clone();
                if let Some(slash) = e.find('/') {
                    let (a, b) = (&e[..slash], &e[slash + 1..]);
                    if !a.is_empty() && !b.is_empty() {
                        t = format!("{a}-{max_r}/{b}");
                    }
                }
                m = step_search(&t);
                start_with_step = m.is_some();
            }

            if let Some((mut low, mut high, step_opt)) = m {
                if field_index == DAY_FIELD && high == "l" {
                    high = "31".to_string();
                }
                if !only_int(&low) {
                    low = alphaconv(field_index, &low, &expressions)?;
                }
                if !only_int(&high) {
                    high = alphaconv(field_index, &high, &expressions)?;
                }
                let step_str = step_opt.unwrap_or_else(|| "1".to_string());
                if !only_int(&step_str) {
                    return Err(CroniterError::BadCron(format!(
                        "[{expr_format}] step '{step_str}' in field {field_index} is not acceptable"
                    )));
                }
                // `int(step)`, croniter.py:1091. Guaranteed to parse: only_int just verified it.
                let step: i64 = step_str
                    .parse()
                    .expect("only_int guarantees a valid decimal integer");
                if step == 0 {
                    return Err(CroniterError::BadCron(format!(
                        "[{expr_format}] step '{step}' in field {field_index} is not acceptable"
                    )));
                }
                for band in [&low, &high] {
                    if !only_int(band) {
                        return Err(CroniterError::BadCron(format!(
                            "[{expr_format}] bands '{low}-{high}' in field {field_index} are not acceptable"
                        )));
                    }
                }
                let low_i: i64 = low
                    .parse()
                    .expect("only_int guarantees a valid decimal integer");
                let high_i: i64 = high
                    .parse()
                    .expect("only_int guarantees a valid decimal integer");
                let low_v = value_alias(low_i, field_index, expressions.len());
                let high_v = value_alias(high_i, field_index, expressions.len());

                if low_v.max(high_v) > min_r.max(max_r) {
                    return Err(CroniterError::BadCron(format!(
                        "{expr_format} is out of bands"
                    )));
                }

                // croniter.py:1130 - "{start}/{step}" collides with an explicit equal range
                // ("Jan-Jan") once normalized; recognising start==end-after-normalizing here
                // is what keeps a literal max/step token (e.g. "59/15") from re-expanding to
                // the whole cycle.
                let start_at_field_max = start_with_step && low_v == high_v;

                let mut low_final = low_v;
                // croniter.py:1136 - Python's `if from_timestamp` is falsy for 0.0, so an
                // exact-epoch start deliberately skips the rebase below.
                if let Some(ts) = from_timestamp {
                    if ts != 0.0 && !start_at_field_max {
                        low_final = get_low_from_current_date_number(
                            field_index,
                            step,
                            ts,
                            from_timestamp_tz,
                        )?;
                    }
                }

                let rng: Vec<i64> = if start_at_field_max {
                    vec![low_final]
                } else if low_final > high_v {
                    let mut rng = num_range_step(low_final, max_r, step);
                    let mut to_skip = 0i64;
                    if let Some(&last) = rng.last() {
                        let whole_len = max_r - min_r + 1;
                        let curpos = last - min_r;
                        let already_skipped = max_r - last;
                        if (curpos + step) > whole_len && already_skipped < step {
                            to_skip = step - already_skipped;
                        }
                    }
                    rng.extend(num_range_step(min_r + to_skip, high_v, step));
                    rng
                } else if low_final == high_v {
                    // An explicit equal range ("Jan-Jan", "Sun-Sun") means the whole cycle.
                    num_range_step(min_r, max_r, step)
                } else {
                    num_range_step(low_final, high_v, step)
                };

                let rng_tokens: Vec<String> = if field_index == DOW_FIELD {
                    if let Some(NthKind::Num(n)) = nth {
                        rng.iter().map(|item| format!("{item}#{n}")).collect()
                    } else {
                        rng.iter().map(|item| item.to_string()).collect()
                    }
                } else {
                    rng.iter().map(|item| item.to_string()).collect()
                };
                for tok in &rng_tokens {
                    if !seen.contains(tok) {
                        e_list.push(tok.clone());
                    }
                }
                seen.extend(rng_tokens);
            } else {
                if t.starts_with('-') {
                    return Err(CroniterError::BadCron(format!(
                        "[{expr_format}] is not acceptable, negative numbers not allowed"
                    )));
                }
                let mut tok = t.clone();
                if !star_or_int(&tok) {
                    tok = alphaconv(field_index, &tok, &expressions)?;
                }

                let final_expr = if tok == "*" {
                    Expr::Star
                } else if tok == "l" {
                    Expr::Last
                } else {
                    let v: i64 = tok.parse().map_err(|_| {
                        CroniterError::Other(format!(
                            "invalid literal for int() with base 10: '{tok}'"
                        ))
                    })?;
                    let v = value_alias(v, field_index, expressions.len());
                    if v < min_r || v > max_r {
                        return Err(CroniterError::BadCron(format!(
                            "[{expr_format}] is not acceptable, out of range"
                        )));
                    }
                    Expr::Num(v)
                };
                res.push(final_expr);

                if field_index == DOW_FIELD {
                    if let (Some(nk), Expr::Num(key)) = (nth, final_expr) {
                        let nval = match nk {
                            NthKind::Num(n) => n,
                            NthKind::Last => NTH_LAST,
                        };
                        nth_weekday_of_month.entry(key).or_default().insert(nval);
                    }
                }
            }
        }

        // Dedup (Python: `set(res)`) then sort with croniter's key. croniter.py:1207-1208.
        let mut uniq: Vec<Expr> = Vec::new();
        for r in res {
            if !uniq.contains(&r) {
                uniq.push(r);
            }
        }
        uniq.sort_by_key(sort_key);

        if uniq.len() == LEN_MEANS_ALL[field_index] {
            // Vixie-cron-bug preservation, croniter.py:1210-1216: an enumerated day-of-month
            // or day-of-week that happens to cover the whole range only collapses to "*" if
            // the *other* of the two fields is also explicitly "*".
            let skip_collapse = (field_index == DAY_FIELD && !expressions[DOW_FIELD].contains('*'))
                || (field_index == DOW_FIELD && !expressions[DAY_FIELD].contains('*'));
            if !skip_collapse {
                uniq = vec![Expr::Star];
            }
        }
        let field_result = if uniq.len() == 1 && uniq[0] == Expr::Star {
            vec![Expr::Star]
        } else {
            uniq
        };
        expanded.push(field_result);
    }

    // croniter.py:1220-1231.
    if !nth_weekday_of_month.is_empty() {
        let dow_vals: BTreeSet<i64> = expanded[DOW_FIELD]
            .iter()
            .filter_map(|e| e.as_num())
            .collect();
        let nth_keys: BTreeSet<i64> = nth_weekday_of_month.keys().copied().collect();
        let remaining: BTreeSet<i64> = dow_vals.difference(&nth_keys).copied().collect();
        if !remaining.is_empty() && expanded[DOW_FIELD].len() != LEN_MEANS_ALL[DOW_FIELD] {
            // croniter: Python set/dict repr ordering for small ints is CPython-internal
            // (hash-table position); this reproduces the common case (sorted) rather than
            // guaranteeing byte-identical output for every possible collision pattern.
            return Err(CroniterError::UnsupportedSyntax(format!(
                "day-of-week field does not support mixing literal values and nth day of week syntax.  Cron: '{expr_format}'    dow={} vs nth={}",
                format_int_set(&remaining),
                format_nth_map(&nth_weekday_of_month)
            )));
        }
    }

    Ok(Expanded {
        fields: expanded,
        nth_weekday_of_month,
        expressions,
        nearest_weekday: nearest_weekday_flag,
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn f(expanded: &Expanded, idx: usize) -> &[Expr] {
        &expanded.fields[idx]
    }

    #[test]
    fn five_field_star() {
        let e = expand("* * * * *", None, false, None, None).unwrap();
        assert_eq!(e.len(), 5);
        for i in 0..5 {
            assert_eq!(f(&e, i), &[Expr::Star]);
        }
    }

    #[test]
    fn five_field_explicit() {
        let e = expand("0 0 1 1 0", None, false, None, None).unwrap();
        assert_eq!(f(&e, MINUTE_FIELD), &[Expr::Num(0)]);
        assert_eq!(f(&e, HOUR_FIELD), &[Expr::Num(0)]);
        assert_eq!(f(&e, DAY_FIELD), &[Expr::Num(1)]);
        assert_eq!(f(&e, MONTH_FIELD), &[Expr::Num(1)]);
        assert_eq!(f(&e, DOW_FIELD), &[Expr::Num(0)]);
    }

    #[test]
    fn six_field_with_seconds() {
        let e = expand("0 0 * * * */15", None, false, None, None).unwrap();
        assert_eq!(e.len(), 6);
        assert_eq!(
            f(&e, SECOND_FIELD),
            &[Expr::Num(0), Expr::Num(15), Expr::Num(30), Expr::Num(45)]
        );
    }

    #[test]
    fn seven_field_with_year() {
        let e = expand("0 0 1 1 * 0 2024", None, false, None, None).unwrap();
        assert_eq!(e.len(), 7);
        assert_eq!(f(&e, YEAR_FIELD), &[Expr::Num(2024)]);
    }

    #[test]
    fn range() {
        let e = expand("0-5,10 * * * mon-fri", None, false, None, None).unwrap();
        assert_eq!(
            f(&e, MINUTE_FIELD),
            &[
                Expr::Num(0),
                Expr::Num(1),
                Expr::Num(2),
                Expr::Num(3),
                Expr::Num(4),
                Expr::Num(5),
                Expr::Num(10)
            ]
        );
        assert_eq!(
            f(&e, DOW_FIELD),
            &[
                Expr::Num(1),
                Expr::Num(2),
                Expr::Num(3),
                Expr::Num(4),
                Expr::Num(5)
            ]
        );
    }

    #[test]
    fn step() {
        let e = expand("*/15 * * * *", None, false, None, None).unwrap();
        assert_eq!(
            f(&e, MINUTE_FIELD),
            &[Expr::Num(0), Expr::Num(15), Expr::Num(30), Expr::Num(45)]
        );
    }

    #[test]
    fn list() {
        let e = expand("1,2,3 * * * *", None, false, None, None).unwrap();
        assert_eq!(
            f(&e, MINUTE_FIELD),
            &[Expr::Num(1), Expr::Num(2), Expr::Num(3)]
        );
    }

    #[test]
    fn month_and_dow_names() {
        let e = expand("0 0 1 JAN MON", None, false, None, None).unwrap();
        assert_eq!(f(&e, MONTH_FIELD), &[Expr::Num(1)]);
        assert_eq!(f(&e, DOW_FIELD), &[Expr::Num(1)]);
    }

    #[test]
    fn at_aliases() {
        let daily = expand("@daily", None, false, None, None).unwrap();
        let plain = expand("0 0 * * *", None, false, None, None).unwrap();
        assert_eq!(daily, plain);

        let hourly = expand("@hourly", None, false, None, None).unwrap();
        assert_eq!(f(&hourly, MINUTE_FIELD), &[Expr::Num(0)]);
        assert_eq!(f(&hourly, HOUR_FIELD), &[Expr::Star]);
    }

    #[test]
    fn at_alias_with_hash_id() {
        let e = expand("@daily", Some(b"abc"), false, None, None).unwrap();
        // "h h * * * h" -> 6 fields, hour/minute/second hashed, day/month/dow star.
        assert_eq!(e.len(), 6);
        assert_eq!(f(&e, DAY_FIELD), &[Expr::Star]);
    }

    #[test]
    fn last_day_of_month() {
        let e = expand("0 * l * *", None, false, None, None).unwrap();
        assert_eq!(f(&e, DAY_FIELD), &[Expr::Last]);
    }

    #[test]
    fn nearest_weekday() {
        let e = expand("0 0 15w * *", None, false, None, None).unwrap();
        assert_eq!(f(&e, DAY_FIELD), &[Expr::Num(15)]);
        assert!(e.nearest_weekday);
    }

    #[test]
    fn nth_weekday_of_month() {
        let e = expand("* * * * mon#2", None, false, None, None).unwrap();
        assert_eq!(f(&e, DOW_FIELD), &[Expr::Num(1)]);
        let mut expect = BTreeSet::new();
        expect.insert(2i64);
        assert_eq!(e.nth_weekday_of_month.get(&1), Some(&expect));
    }

    #[test]
    fn last_weekday_of_month() {
        let e = expand("* * * * l3", None, false, None, None).unwrap();
        assert_eq!(f(&e, DOW_FIELD), &[Expr::Num(3)]);
        let mut expect = BTreeSet::new();
        expect.insert(NTH_LAST);
        assert_eq!(e.nth_weekday_of_month.get(&3), Some(&expect));
    }

    #[test]
    fn question_mark_becomes_star() {
        let e = expand("0 0 ? * *", None, false, None, None).unwrap();
        assert_eq!(f(&e, DAY_FIELD), &[Expr::Star]);
    }

    #[test]
    fn hash_expression() {
        let e = expand("h h * * *", Some(b"myid"), false, None, None).unwrap();
        // Deterministic given the same hash_id: must be a single in-range value.
        match f(&e, MINUTE_FIELD) {
            [Expr::Num(n)] => assert!((0..=59).contains(n)),
            other => panic!("unexpected {other:?}"),
        }
        let e2 = expand("h h * * *", Some(b"myid"), false, None, None).unwrap();
        assert_eq!(
            e, e2,
            "hash expansion must be deterministic for the same hash_id"
        );
    }

    #[test]
    fn hash_with_range_and_divisor() {
        let e = expand("h(0-29)/10 * * * *", Some(b"x"), false, None, None).unwrap();
        let vals = f(&e, MINUTE_FIELD);
        assert!(
            vals.iter().all(
                |v| matches!(v, Expr::Num(n) if (0..30).contains(n)) || matches!(v, Expr::Star)
            )
        );
    }

    #[test]
    fn random_expression_is_in_range() {
        let e = expand("r r * * *", None, false, None, None).unwrap();
        match f(&e, MINUTE_FIELD) {
            [Expr::Num(n)] => assert!((0..=59).contains(n)),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn out_of_range_rejected() {
        let err = expand("60 * * * *", None, false, None, None).unwrap_err();
        assert_eq!(
            err,
            CroniterError::BadCron("[60 * * * *] is not acceptable, out of range".to_string())
        );
    }

    #[test]
    fn err_wrong_field_count() {
        let err = expand("* * * *", None, false, None, None).unwrap_err();
        assert_eq!(
            err,
            CroniterError::BadCron(
                "Exactly 5, 6 or 7 columns has to be specified for iterator expression."
                    .to_string()
            )
        );
    }

    #[test]
    fn err_negative_number() {
        let err = expand("-5 * * * *", None, false, None, None).unwrap_err();
        assert_eq!(
            err,
            CroniterError::BadCron(
                "[-5 * * * *] is not acceptable, negative numbers not allowed".to_string()
            )
        );
    }

    #[test]
    fn err_bad_alpha() {
        let err = expand("0 0 1 FOO *", None, false, None, None).unwrap_err();
        assert_eq!(
            err,
            CroniterError::NotAlpha("[0 0 1 foo *] is not acceptable".to_string())
        );
    }

    #[test]
    fn err_zero_step() {
        let err = expand("*/0 * * * *", None, false, None, None).unwrap_err();
        assert_eq!(
            err,
            CroniterError::BadCron(
                "[*/0 * * * *] step '0' in field 0 is not acceptable".to_string()
            )
        );
    }

    #[test]
    fn err_hash_without_hash_id() {
        let err = expand("h * * * *", None, false, None, None).unwrap_err();
        assert_eq!(
            err,
            CroniterError::BadCron("Hashed definitions must include hash_id".to_string())
        );
    }

    #[test]
    fn err_question_mark_with_other_chars() {
        let err = expand("0 0 ?,1 * *", None, false, None, None).unwrap_err();
        assert_eq!(
            err,
            CroniterError::BadCron(
                "[0 0 ?,1 * *] is not acceptable. Question mark can not used with other characters"
                    .to_string()
            )
        );
    }

    #[test]
    fn err_mixing_nth_and_literal_dow() {
        let err = expand("* * * * 2,mon#2", None, false, None, None).unwrap_err();
        assert!(matches!(err, CroniterError::UnsupportedSyntax(_)));
    }

    #[test]
    fn out_of_bands_message_has_no_brackets() {
        let err = expand("100-200 * * * *", None, false, None, None).unwrap_err();
        assert_eq!(
            err,
            CroniterError::BadCron("100-200 * * * * is out of bands".to_string())
        );
    }
}
