//! Typed errors of goway itself and the exit codes they map to.
//!
//! The exit code contract (frob records it as evidence): a remote command's
//! own exit code passes through unchanged, 128+N when it died of signal N,
//! and [`EXIT_GOWAY_FAILURE`] when goway could not run the command at all.

use std::path::PathBuf;

/// Exit code when goway itself fails, borrowed from `docker run` (125).
pub const EXIT_GOWAY_FAILURE: u8 = 125;

/// Every failure goway can report; each variant renders as one clear message.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A file goway owns could not be read or written.
    #[error("cannot {action} {path}: {source}")]
    Io {
        /// What goway tried to do, such as "read" or "create".
        action: &'static str,
        /// The path involved.
        path: PathBuf,
        /// The underlying OS error.
        source: std::io::Error,
    },
    /// The config file is malformed or inconsistent.
    #[error("config {path}: {message}")]
    Config {
        /// The config file.
        path: PathBuf,
        /// What is wrong, from the parser or validation.
        message: String,
    },
    /// The cached host state file is malformed.
    #[error("state {path}: {message}")]
    State {
        /// The state file.
        path: PathBuf,
        /// What is wrong.
        message: String,
    },
    /// No configured host has this name.
    #[error("no host named `{0}` in the config; add it with `goway host add {0}`")]
    UnknownHost(String),
    /// A command line feature that is planned but not built yet.
    #[error("`{0}` is not implemented yet")]
    NotImplemented(&'static str),
}

impl Error {
    /// The process exit code for this error; always [`EXIT_GOWAY_FAILURE`].
    pub fn exit_code(&self) -> u8 {
        EXIT_GOWAY_FAILURE
    }

    /// Build an [`Error::Io`] for `action` on `path`.
    pub fn io(action: &'static str, path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            action,
            path: path.into(),
            source,
        }
    }
}

/// goway's result alias.
pub type Result<T, E = Error> = std::result::Result<T, E>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_goway_error_exits_125() {
        let errors = [
            Error::NotImplemented("gc"),
            Error::UnknownHost("q".to_owned()),
            Error::io("read", "/x", std::io::Error::other("boom")),
        ];
        for e in errors {
            assert_eq!(e.exit_code(), 125, "{e}");
        }
    }
}
