//! One error type for the whole database layer.

use std::fmt;

#[derive(Debug)]
pub enum Error {
    /// The database stayed locked after every retry.
    Busy,
    /// A constraint (CHECK, UNIQUE, FOREIGN KEY, trigger) rejected the write.
    Constraint(String),
    /// A row the caller named does not exist.
    NotFound(String),
    /// The caller passed a value the repository refuses (Python's `ValueError`).
    Invalid(String),
    /// The requested state change is not legal from the current state.
    Illegal(String),
    /// The expected version or fencing token is stale, or the lease is gone.
    Conflict(String),
    /// Any other SQLite failure.
    Sqlite(rusqlite::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Busy => f.write_str("database is locked"),
            Self::Constraint(message)
            | Self::NotFound(message)
            | Self::Invalid(message)
            | Self::Illegal(message)
            | Self::Conflict(message) => f.write_str(message),
            Self::Sqlite(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<rusqlite::Error> for Error {
    fn from(error: rusqlite::Error) -> Self {
        use rusqlite::ErrorCode::{ConstraintViolation, DatabaseBusy, DatabaseLocked};
        match error.sqlite_error_code() {
            Some(DatabaseBusy | DatabaseLocked) => Self::Busy,
            Some(ConstraintViolation) => Self::Constraint(error.to_string()),
            _ => Self::Sqlite(error),
        }
    }
}
