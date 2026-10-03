//! The install, uninstall and status commands, generic over the `System` they act on.

use std::path::{Path, PathBuf};
use std::time::Duration;

use goway_journal::{
    ApplyError, Change, Journal, JournalError, Outcome, System, apply_with, revert, still_applied,
};

use crate::error::SetupError;
use crate::host::HostSettings;
use crate::layout::Layout;

/// How hard uninstall retries a revert that fails on locked files.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Retry {
    /// Total attempts, at least one.
    pub attempts: u32,
    /// Pause between attempts.
    pub delay: Duration,
}

impl Retry {
    /// The default for a relaunched uninstaller waiting for its parent to exit.
    pub const PATIENT: Self = Self {
        attempts: 40,
        delay: Duration::from_millis(250),
    };
    /// A single attempt.
    pub const ONCE: Self = Self {
        attempts: 1,
        delay: Duration::ZERO,
    };
}

/// One journal entry as `status` reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusRow {
    /// Position in the journal.
    pub index: usize,
    /// The recorded change.
    pub change: Change,
    /// Whether the entry has already been reverted.
    pub reverted: bool,
    /// Whether the target currently holds what goway wrote.
    pub holds: bool,
}

/// What an uninstall did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UninstallReport {
    /// The journal as reverted.
    pub journal: Journal,
    /// `(entry index, outcome)` in replay order.
    pub outcomes: Vec<(usize, Outcome)>,
}

/// Persist the host settings so a later uninstall reaches the same distro.
pub fn save_settings(layout: &Layout, settings: &HostSettings) -> Result<(), SetupError> {
    std::fs::create_dir_all(&layout.state_dir).map_err(|e| SetupError::io(&layout.state_dir, e))?;
    let json = serde_json::to_string_pretty(settings).map_err(JournalError::from)?;
    std::fs::write(&layout.host_settings_path, json)
        .map_err(|e| SetupError::io(&layout.host_settings_path, e))
}

/// Read the saved host settings; `None` when there are none.
pub fn load_settings(layout: &Layout) -> Result<Option<HostSettings>, SetupError> {
    match std::fs::read_to_string(&layout.host_settings_path) {
        Ok(text) => Ok(serde_json::from_str(&text).ok()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(SetupError::io(&layout.host_settings_path, e)),
    }
}

/// Delete the saved host settings, then the state directory if nothing else is in it.
pub fn remove_settings(layout: &Layout) {
    if let Err(e) = std::fs::remove_file(&layout.host_settings_path) {
        tracing::debug!(path = %layout.host_settings_path.display(), error = %e, "no settings to remove");
    }
    let _ = std::fs::remove_dir(&layout.state_dir);
}

/// Load the profile's journal; `None` when it has none.
pub fn load_journal(layout: &Layout) -> Result<Option<Journal>, SetupError> {
    if !layout.journal_path.exists() {
        return Ok(None);
    }
    Ok(Some(Journal::load(&layout.journal_path)?))
}

/// Refuse when the journal records a live (not fully reverted) install.
pub fn ensure_not_installed(layout: &Layout) -> Result<(), SetupError> {
    if let Some(existing) = load_journal(layout)? {
        if existing.entries.iter().any(|e| !e.reverted) {
            tracing::warn!(profile = %layout.profile, "install refused: journal records a live install");
            return Err(SetupError::AlreadyInstalled {
                profile: layout.profile.clone(),
                journal: layout.journal_path.display().to_string(),
            });
        }
        tracing::info!(profile = %layout.profile, "replacing a fully reverted journal");
    }
    Ok(())
}

/// Apply `plan`, persisting the journal after every entry; roll back on failure.
pub fn install(
    sys: &mut (impl System + ?Sized),
    layout: &Layout,
    plan: &[Change],
) -> Result<Journal, SetupError> {
    ensure_not_installed(layout)?;
    std::fs::create_dir_all(&layout.state_dir).map_err(|e| SetupError::io(&layout.state_dir, e))?;
    let path = layout.journal_path.clone();
    let result = apply_with(plan, sys, Journal::generate(), &mut |j| j.save(&path));
    match result {
        Ok(journal) => Ok(journal),
        Err(ApplyError {
            index,
            source,
            mut journal,
        }) => {
            tracing::error!(index, error = %source, "install failed; rolling back");
            let rollback = revert(&mut journal, sys).and_then(|_| journal.save(&path));
            match rollback {
                Ok(()) => {
                    finish_journal(layout);
                    Err(SetupError::InstallFailed { index, source })
                }
                Err(rollback) => Err(SetupError::RollbackFailed {
                    install: source.to_string(),
                    rollback,
                }),
            }
        }
    }
}

/// Revert the profile's journal, retrying locked-file failures, then delete the journal.
///
/// `Ok(None)` means there was no journal. Entries whose targets were edited since install are
/// left alone and listed in the report; the journal is deleted once every entry is settled.
pub fn uninstall(
    sys: &mut (impl System + ?Sized),
    layout: &Layout,
    retry: Retry,
) -> Result<Option<UninstallReport>, SetupError> {
    let Some(mut journal) = load_journal(layout)? else {
        tracing::info!(profile = %layout.profile, "nothing to uninstall");
        return Ok(None);
    };
    let mut attempt = 1;
    let report = loop {
        let result = revert(&mut journal, sys);
        journal.save(&layout.journal_path)?;
        match result {
            Ok(report) => break report,
            Err(JournalError::System(e)) if attempt < retry.attempts => {
                tracing::warn!(attempt, error = %e, "revert failed; retrying");
                std::thread::sleep(retry.delay);
                attempt += 1;
            }
            Err(e) => return Err(e.into()),
        }
    };
    finish_journal(layout);
    Ok(Some(UninstallReport {
        journal,
        outcomes: report.outcomes,
    }))
}

/// Delete the journal file, then its directory if nothing else is in it.
fn finish_journal(layout: &Layout) {
    if let Err(e) = std::fs::remove_file(&layout.journal_path) {
        tracing::warn!(path = %layout.journal_path.display(), error = %e, "could not remove journal");
    }
    match std::fs::remove_dir(&layout.state_dir) {
        Ok(()) => tracing::info!(path = %layout.state_dir.display(), "removed state directory"),
        Err(e) => {
            tracing::debug!(path = %layout.state_dir.display(), error = %e, "kept state directory");
        }
    }
}

/// Report every journal entry and whether its target still holds goway's value.
pub fn status(
    sys: &(impl System + ?Sized),
    journal: &Journal,
) -> Result<Vec<StatusRow>, SetupError> {
    journal
        .entries
        .iter()
        .enumerate()
        .map(|(index, e)| {
            Ok(StatusRow {
                index,
                change: e.change.clone(),
                reverted: e.reverted,
                holds: still_applied(&e.change, sys)?,
            })
        })
        .collect()
}

/// Whether `exe` is one of the files the journal installed (so it cannot delete itself).
pub fn runs_from_installed_file(journal: &Journal, exe: &Path) -> bool {
    let norm = |p: &Path| {
        let s = p.to_string_lossy().replace('/', "\\").to_lowercase();
        s.strip_prefix("\\\\?\\").map_or(s.clone(), str::to_owned)
    };
    let exe = norm(exe);
    journal.entries.iter().any(|e| {
        !e.reverted && matches!(&e.change, Change::InstallFile { path, .. } if norm(path) == exe)
    })
}

/// The directory a relaunched uninstaller copy lives in.
pub fn relaunch_dir(temp: &Path, pid: u32) -> PathBuf {
    temp.join(format!("goway-uninstall-{pid}"))
}
