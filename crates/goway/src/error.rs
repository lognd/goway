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
    /// No candidate address answered with the host's pinned key.
    #[error("cannot reach host `{name}`{}", render_misses(misses))]
    HostNotFound {
        /// The host name.
        name: String,
        /// Every candidate tried and why it failed.
        misses: Vec<crate::resolve::Miss>,
    },
    /// `goway host add` found no usable Linux sshd for the host.
    #[error("cannot add host `{name}`: {reason}")]
    HostAdd {
        /// The host name.
        name: String,
        /// Why, with the next step.
        reason: String,
    },
    /// A command line feature that is planned but not built yet.
    #[error("`{0}` is not implemented yet")]
    NotImplemented(&'static str),
}

fn render_misses(misses: &[crate::resolve::Miss]) -> String {
    if misses.is_empty() {
        return ": no candidate address (no cache, no `address`, the name and NAME.local did not resolve)".to_owned();
    }
    let mut out = String::from("; tried:");
    for m in misses {
        out.push_str("\n  ");
        out.push_str(&m.to_string());
    }
    if misses
        .iter()
        .any(|m| m.failure == crate::ssh::Failure::HostKeyUnknown)
    {
        out.push_str("\n  hint: pin the key first with `goway host add NAME`");
    }
    out
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
