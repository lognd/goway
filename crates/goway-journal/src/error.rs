//! Typed errors for the journal crate.

use std::path::PathBuf;

use crate::journal::Journal;

/// A failure of one primitive `System` operation.
#[derive(Debug, thiserror::Error)]
pub enum SystemError {
    /// The target of a read-modify operation does not exist.
    #[error("not found: {0}")]
    NotFound(String),
    /// The operation is not implemented on this platform or system.
    #[error("unsupported on this platform: {0}")]
    Unsupported(&'static str),
    /// The target is in a state the operation refuses to act on (for example a non-empty directory).
    #[error("invalid state: {0}")]
    InvalidState(String),
    /// An external command (PowerShell, `wsl.exe`) could not run or reported failure.
    #[error("command failed: {what}: {detail}")]
    Command {
        /// What was being attempted.
        what: String,
        /// Exit status and the command's own error text.
        detail: String,
    },
    /// An operating-system I/O failure.
    #[error("i/o error on {path}: {source}")]
    Io {
        /// The path being operated on.
        path: PathBuf,
        /// The underlying error.
        source: std::io::Error,
    },
}

/// A failure while planning, applying, reverting or persisting changes.
#[derive(Debug, thiserror::Error)]
pub enum JournalError {
    /// A primitive system operation failed.
    #[error(transparent)]
    System(#[from] SystemError),
    /// The change itself is malformed (for example a line containing a newline).
    #[error("invalid change: {0}")]
    Invalid(String),
    /// Journal file I/O failed.
    #[error("journal i/o error on {path}: {source}")]
    Io {
        /// The journal path.
        path: PathBuf,
        /// The underlying error.
        source: std::io::Error,
    },
    /// Journal (de)serialization failed.
    #[error("journal json error: {0}")]
    Json(#[from] serde_json::Error),
}

/// An apply that stopped part-way; carries the journal so the partial work can be reverted.
#[derive(Debug, thiserror::Error)]
#[error("change {index} failed: {source}")]
pub struct ApplyError {
    /// Index in the plan of the failing change.
    pub index: usize,
    /// Why it failed.
    #[source]
    pub source: JournalError,
    /// The journal as recorded so far (the failing change is recorded, write-ahead).
    pub journal: Box<Journal>,
}
