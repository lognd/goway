//! Typed errors for the installer.

use goway_journal::JournalError;

/// Everything that can make an install, uninstall or status command fail.
#[derive(Debug, thiserror::Error)]
pub enum SetupError {
    /// The profile name is empty or contains characters unsafe in paths and registry keys.
    #[error("invalid profile name {0:?}: use letters, digits, '.', '-' and '_'")]
    BadProfile(String),
    /// The per-user local application data directory could not be determined.
    #[error("cannot determine the local application data directory")]
    NoLocalAppData,
    /// A journal for this profile already records an install.
    #[error(
        "profile {profile} is already installed (journal {journal}); run `goway-setup uninstall --profile {profile}` first"
    )]
    AlreadyInstalled {
        /// Profile name.
        profile: String,
        /// Journal path.
        journal: String,
    },
    /// This build carries no goway.exe payload.
    #[error(
        "this goway-setup was built without a goway.exe payload; build it with scripts/windows/build.sh"
    )]
    NoPayload,
    /// Applying the plan failed; the partial work was rolled back.
    #[error("install failed at change {index} and was rolled back: {source}")]
    InstallFailed {
        /// Index of the failing change.
        index: usize,
        /// Why it failed.
        source: JournalError,
    },
    /// Applying the plan failed and the rollback failed too; the journal still records the work.
    #[error(
        "install failed ({install}) and rolling back failed ({rollback}); run `goway-setup uninstall` to retry"
    )]
    RollbackFailed {
        /// The install failure.
        install: String,
        /// The rollback failure.
        rollback: JournalError,
    },
    /// A journal operation failed.
    #[error(transparent)]
    Journal(#[from] JournalError),
    /// A filesystem operation outside the journal failed.
    #[error("i/o error on {path}: {source}")]
    Io {
        /// The path involved.
        path: String,
        /// The underlying error.
        source: std::io::Error,
    },
}

impl SetupError {
    /// Wrap an I/O error with the path it concerns.
    pub fn io(path: &std::path::Path, source: std::io::Error) -> Self {
        Self::Io {
            path: path.display().to_string(),
            source,
        }
    }
}
