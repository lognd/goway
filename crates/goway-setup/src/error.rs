//! Typed errors for the installer.

use goway_journal::{JournalError, SystemError};

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
    /// The WSL distro name is empty or contains characters unsafe in command lines.
    #[error("invalid distro name {0:?}: use letters, digits, '.', '-' and '_'")]
    BadDistro(String),
    /// The host component changes Windows and WSL and only runs on Windows.
    #[error("the host component only runs on Windows (use --dry-run to see its plan)")]
    HostNeedsWindows,
    /// The host component needs administrator rights and cannot get them.
    #[error("the host component needs administrator rights (firewall rules): {0}")]
    NeedsAdmin(String),
    /// The WSL distro is missing or does not start.
    #[error("WSL distro {0} did not respond; check `wsl -l -v` and pass --distro NAME")]
    DistroUnreachable(String),
    /// systemd is not running in the distro, so sshd cannot be managed.
    #[error(
        "systemd is not running in WSL distro {distro}; enable it, then run the install again:\n  wsl -d {distro} -u root --exec sh -c \"printf '[boot]\\nsystemd=true\\n' >> /etc/wsl.conf\"\n  wsl --terminate {distro}"
    )]
    SystemdOff {
        /// The distro.
        distro: String,
    },
    /// State an elevated process would act on sits where a non-administrator could have written it.
    #[error("refusing to trust {path}: {reason}")]
    UntrustedState {
        /// The directory, file or entry that was refused.
        path: String,
        /// Why it was refused.
        reason: String,
    },
    /// The client component is per-user and must never run with an elevated token.
    #[error("the client component never runs elevated; only the host component does")]
    ClientNeverElevated,
    /// The elevated re-run failed; its own output was printed above.
    #[error("the elevated run failed with exit code {0}; its output is shown above")]
    ElevatedRunFailed(u32),
    /// A probe or activation command against the machine failed.
    #[error(transparent)]
    System(#[from] SystemError),
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
