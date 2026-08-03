use thiserror::Error;

/// Mirrors croniter's exception hierarchy.
///
/// Python has real subclassing: `CroniterError(ValueError)`,
/// `CroniterBadCronError(CroniterError)`, `CroniterUnsupportedSyntaxError(CroniterBadCronError)`,
/// `CroniterNotAlphaError(CroniterBadCronError)`, `CroniterBadDateError(CroniterError)`,
/// `CroniterBadTypeRangeError(TypeError)`.
///
/// Rust has no inheritance, so the hierarchy is flattened into one enum and the
/// is-a relationships are recovered by [`CroniterError::is_bad_cron`] and friends.
/// Tests that assert on a specific Python class compare against [`CroniterError::class_name`].
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CroniterError {
    /// `CroniterBadCronError` - the expression is not a valid cron expression.
    #[error("{0}")]
    BadCron(String),

    /// `CroniterUnsupportedSyntaxError` - valid cron, syntax croniter refuses to support.
    #[error("{0}")]
    UnsupportedSyntax(String),

    /// `CroniterNotAlphaError` - an alphabetic token was used in a field that has no alphabet.
    #[error("{0}")]
    NotAlpha(String),

    /// `CroniterBadDateError` - no matching date within `max_years_between_matches`.
    #[error("{0}")]
    BadDate(String),

    /// `CroniterBadTypeRangeError` - a value fell outside the representable datetime range.
    #[error("{0}")]
    BadTypeRange(String),

    /// A bare `ValueError`. croniter raises this directly for argument misuse rather
    /// than through its own hierarchy. Passing `start_time` to `get_next` while
    /// `expand_from_start_time` is set is the one case (croniter.py:347-350). Kept
    /// distinct from [`Self::Other`] because the class name is part of the contract:
    /// callers catch `ValueError`, and `CroniterError` is a *subclass* of it, so
    /// reporting the wrong one inverts the relationship.
    #[error("{0}")]
    Value(String),

    /// Bare `CroniterError`, plus the `TypeError` cases croniter raises directly.
    #[error("{0}")]
    Other(String),
}

impl CroniterError {
    /// The Python exception class name, for conformance comparison.
    pub fn class_name(&self) -> &'static str {
        match self {
            Self::BadCron(_) => "CroniterBadCronError",
            Self::UnsupportedSyntax(_) => "CroniterUnsupportedSyntaxError",
            Self::NotAlpha(_) => "CroniterNotAlphaError",
            Self::BadDate(_) => "CroniterBadDateError",
            Self::BadTypeRange(_) => "CroniterBadTypeRangeError",
            Self::Value(_) => "ValueError",
            Self::Other(_) => "CroniterError",
        }
    }

    /// True where Python's `isinstance(exc, CroniterBadCronError)` is true.
    pub fn is_bad_cron(&self) -> bool {
        matches!(
            self,
            Self::BadCron(_) | Self::UnsupportedSyntax(_) | Self::NotAlpha(_)
        )
    }

    /// True where Python's `isinstance(exc, CroniterError)` is true.
    ///
    /// `BadTypeRange` is excluded because in Python it descends from `TypeError`, and
    /// `Value` because a bare `ValueError` is the *parent* of `CroniterError`, not an
    /// instance of it.
    pub fn is_croniter_error(&self) -> bool {
        !matches!(self, Self::BadTypeRange(_) | Self::Value(_))
    }
}

pub type Result<T> = std::result::Result<T, CroniterError>;
