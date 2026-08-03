use std::collections::{BTreeMap, BTreeSet};

/// Field indices, matching croniter's module-level constants exactly.
pub const MINUTE_FIELD: usize = 0;
pub const HOUR_FIELD: usize = 1;
pub const DAY_FIELD: usize = 2;
pub const MONTH_FIELD: usize = 3;
pub const DOW_FIELD: usize = 4;
pub const SECOND_FIELD: usize = 5;
pub const YEAR_FIELD: usize = 6;

pub const UNIX_CRON_LEN: usize = 5;
pub const SECOND_CRON_LEN: usize = 6;
pub const YEAR_CRON_LEN: usize = 7;

/// Inclusive (low, high) bounds per field. croniter's `croniter.RANGES`.
pub const RANGES: [(i64, i64); 7] = [
    (0, 59),
    (0, 23),
    (1, 31),
    (1, 12),
    (0, 6),
    (0, 59),
    (1970, 2099),
];

/// A field's expansion count that means "every value", croniter's `LEN_MEANS_ALL`.
pub const LEN_MEANS_ALL: [usize; 7] = [60, 24, 31, 12, 7, 60, 130];

pub const DAYS: [u32; 12] = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];

/// One entry in an expanded field.
///
/// croniter's `ExpandedExpression = list[Union[int, Literal["*", "l"]]]`. The `"l"`
/// literal only ever appears in the day-of-month field, meaning "last day of month".
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Expr {
    Star,
    Last,
    Num(i64),
}

impl Expr {
    pub fn as_num(self) -> Option<i64> {
        match self {
            Self::Num(n) => Some(n),
            _ => None,
        }
    }

    pub fn is_star(self) -> bool {
        matches!(self, Self::Star)
    }
}

/// The parsed form of a cron expression. Output of the parser, input to the search.
///
/// Field order and contents mirror croniter's `_expand` return value one for one, so
/// the search can be read side by side with the Python.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Expanded {
    /// One `Vec<Expr>` per field, in `RANGES` order. Length is 5, 6, or 7.
    ///
    /// Invariant carried over from croniter: a field that matches everything is
    /// exactly `[Expr::Star]`, never an enumerated full range.
    pub fields: Vec<Vec<Expr>>,

    /// `nth_weekday_of_month`: day-of-week -> set of nth occurrences, from `dow#n` syntax.
    /// The `l` suffix (last such weekday in the month) is stored as [`NTH_LAST`].
    pub nth_weekday_of_month: BTreeMap<i64, BTreeSet<i64>>,

    /// The raw per-field expression strings, pre-expansion. croniter keeps these because
    /// the vixie-cron-bug check re-inspects the original text for a leading `*`.
    pub expressions: Vec<String>,

    /// Set by the `W` (nearest weekday) suffix in the day-of-month field.
    pub nearest_weekday: bool,
}

/// Sentinel for `dow#l`, the last such weekday of the month.
pub const NTH_LAST: i64 = -1;

impl Expanded {
    /// Number of fields: 5 (unix), 6 (with seconds), or 7 (with year).
    pub fn len(&self) -> usize {
        self.fields.len()
    }

    pub fn is_empty(&self) -> bool {
        self.fields.is_empty()
    }

    pub fn has_seconds(&self) -> bool {
        self.fields.len() > UNIX_CRON_LEN
    }

    pub fn has_year(&self) -> bool {
        self.fields.len() == YEAR_CRON_LEN
    }
}

pub fn is_leap(year: i32) -> bool {
    year % 400 == 0 || (year % 4 == 0 && year % 100 != 0)
}

pub fn last_day_of_month(year: i32, month: u32) -> u32 {
    let mut last = DAYS[(month - 1) as usize];
    if month == 2 && is_leap(year) {
        last += 1;
    }
    last
}
