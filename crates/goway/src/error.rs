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
    /// git failed or the directory is not a work tree.
    #[error("{message}")]
    Git {
        /// git's complaint or why git could not run.
        message: String,
    },
    /// An ssh call to a resolved host failed.
    #[error("ssh to `{host}` failed: {message}")]
    Ssh {
        /// The host name.
        host: String,
        /// stderr of ssh or the remote script.
        message: String,
    },
    /// No host of the pool can take a job now.
    #[error("no usable host:\n  {}\n  next: `goway status` shows each host's state; `goway add NAME` adds another helper; `goway run --host local -- CMD` runs on this machine, and `[local] fallback = true` in the config does that automatically when no helper is reachable", .0.join("\n  "))]
    NoHost(Vec<String>),
    /// Hosts answered, but none meets the run's `--needs`.
    #[error("no host meets the requirements:\n  {}\n  next: `goway status` shows each host's hardware; relax --needs, or label/add a host that fits", .0.join("\n  "))]
    NeedsUnmet(Vec<String>),
    /// The command line is inconsistent or incomplete.
    #[error("{0}")]
    Usage(String),
    /// A command line feature that is planned but not built yet.
    #[error("`{0}` is not implemented yet")]
    NotImplemented(&'static str),
}

fn render_misses(misses: &[crate::resolve::Miss]) -> String {
    use crate::ssh::Failure;
    if misses.is_empty() {
        return ": no address found for it (no cached address, no `address` in the config, and neither the name nor NAME.local resolved)\n  next: is it switched on, awake and on the same network? check with `goway status`".to_owned();
    }
    let mut out = String::from("; tried:");
    for m in misses {
        out.push_str("\n  ");
        out.push_str(&m.to_string());
    }
    let any = |f: Failure| misses.iter().any(|m| m.failure == f);
    if any(Failure::HostKeyUnknown) {
        out.push_str("\n  next: pin its key first: `goway add NAME` (or `goway host add NAME`)");
    }
    if any(Failure::AuthRefused) {
        out.push_str("\n  next: set up key login: `goway add NAME` (asks for its password once)");
    }
    if any(Failure::HostKeyMismatch) {
        out.push_str("\n  next: that address answered with a different key: another machine has it now (goway skips it), or NAME was reinstalled; if reinstalled: `goway host remove NAME`, then `goway add NAME`");
    }
    if misses.iter().all(|m| m.failure == Failure::Unreachable) {
        out.push_str("\n  next: is it switched on, awake and on the same network, with WSL running? check with `goway status`");
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
            Error::NeedsUnmet(vec!["h: lacks gpu".to_owned()]),
            Error::io("read", "/x", std::io::Error::other("boom")),
        ];
        for e in errors {
            assert_eq!(e.exit_code(), 125, "{e}");
        }
    }
}
