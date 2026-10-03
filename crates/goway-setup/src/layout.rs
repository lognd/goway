//! Where a profile's files, journal and registry entries live.

use std::path::{Path, PathBuf};

use crate::error::SetupError;

/// The default profile name; also the Add/Remove Programs key name.
pub const DEFAULT_PROFILE: &str = "goway";

/// Registry key (under HKCU) listing installed programs.
const UNINSTALL_ROOT: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall";

/// Every location one profile touches, derived from the local application data directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    /// Profile name.
    pub profile: String,
    /// `<local>\Programs`, the parent of all per-user installs.
    pub programs_dir: PathBuf,
    /// `<local>\Programs\<profile>`.
    pub install_root: PathBuf,
    /// `<install_root>\bin`, the directory added to the user PATH.
    pub bin_dir: PathBuf,
    /// `<bin_dir>\goway.exe`.
    pub goway_exe: PathBuf,
    /// `<install_root>\goway-setup.exe`, the copy the uninstall entry runs.
    pub setup_exe: PathBuf,
    /// `<local>\<profile>`, holding the journal.
    pub state_dir: PathBuf,
    /// `<state_dir>\install-journal.json`.
    pub journal_path: PathBuf,
    /// The Add/Remove Programs key for this profile.
    pub uninstall_key: String,
    /// `<state_dir>\host-journal.json`, the host component's journal (one journal per component).
    pub host_journal_path: PathBuf,
    /// `<state_dir>\host-settings.json`, the distro and port the host install used.
    pub host_settings_path: PathBuf,
}

/// Accept only names that are safe as a directory name and as a registry key name.
pub fn validate_profile(profile: &str) -> Result<(), SetupError> {
    let ok = !profile.is_empty()
        && profile != "."
        && profile != ".."
        && profile
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'));
    if ok {
        Ok(())
    } else {
        tracing::warn!(profile, "rejected profile name");
        Err(SetupError::BadProfile(profile.to_owned()))
    }
}

impl Layout {
    /// Derive the layout of `profile` under `local_app_data`.
    pub fn new(local_app_data: &Path, profile: &str) -> Result<Self, SetupError> {
        validate_profile(profile)?;
        let programs_dir = local_app_data.join("Programs");
        let install_root = programs_dir.join(profile);
        let bin_dir = install_root.join("bin");
        let state_dir = local_app_data.join(profile);
        Ok(Self {
            profile: profile.to_owned(),
            goway_exe: bin_dir.join("goway.exe"),
            setup_exe: install_root.join("goway-setup.exe"),
            journal_path: state_dir.join("install-journal.json"),
            uninstall_key: format!(r"{UNINSTALL_ROOT}\{profile}"),
            host_journal_path: state_dir.join("host-journal.json"),
            host_settings_path: state_dir.join("host-settings.json"),
            programs_dir,
            install_root,
            bin_dir,
            state_dir,
        })
    }

    /// This layout with the host component's journal in place of the client's.
    ///
    /// `app::install`, `uninstall` and `load_journal` act on `journal_path`, so the host
    /// component reuses them unchanged through this view.
    #[must_use]
    pub fn host_view(&self) -> Self {
        Self {
            journal_path: self.host_journal_path.clone(),
            ..self.clone()
        }
    }

    /// Derive the layout under the real per-user local application data directory.
    pub fn from_environment(profile: &str) -> Result<Self, SetupError> {
        let local = dirs::data_local_dir().ok_or(SetupError::NoLocalAppData)?;
        Self::new(&local, profile)
    }
}
