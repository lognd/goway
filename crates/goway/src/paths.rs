//! Where goway keeps its local files: config, state and pinned host keys.
//!
//! `GOWAY_CONFIG_DIR` and `GOWAY_STATE_DIR` override the platform defaults
//! (tests rely on them; users can too).

use std::path::PathBuf;

/// Where an install that predates the move to the local profile kept its config on Windows
/// (`%APPDATA%\goway`, which roams to domain servers), and where it lives now
/// (`%LOCALAPPDATA%\goway`): the private key in it must not leave the machine.
///
/// Returns the directory to use: `local` when it exists or nothing needs moving; otherwise the
/// legacy directory is renamed to `local` (the ACL moves with it), and only when that fails does
/// the legacy path stay in use, so a failed move never loses a key.
pub fn migrate_config_dir(legacy: &std::path::Path, local: &std::path::Path) -> PathBuf {
    if legacy == local || local.exists() || !legacy.is_dir() {
        return local.to_path_buf();
    }
    if let Some(parent) = local.parent()
        && let Err(e) = std::fs::create_dir_all(parent)
    {
        tracing::warn!(error = %e, "could not create the local config parent; keeping the roaming directory");
        return legacy.to_path_buf();
    }
    match std::fs::rename(legacy, local) {
        Ok(()) => {
            tracing::info!(from = %legacy.display(), to = %local.display(), "moved the config directory out of the roaming profile");
            local.to_path_buf()
        }
        Err(e) => {
            tracing::warn!(error = %e, "could not move the config directory; keeping the roaming one");
            legacy.to_path_buf()
        }
    }
}

/// The default config directory: `~/.config/goway`, on Windows `%LOCALAPPDATA%\goway` (never
/// the roaming profile), moving an older roaming directory over.
fn default_config_dir() -> PathBuf {
    #[cfg(windows)]
    if let (Some(local), Some(roaming)) = (dirs::config_local_dir(), dirs::config_dir()) {
        return migrate_config_dir(&roaming.join("goway"), &local.join("goway"));
    }
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("goway")
}

/// The local directories goway uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    /// Holds `config.toml` and `known_hosts`.
    pub config_dir: PathBuf,
    /// Holds cached addresses (and control sockets without a runtime dir).
    pub state_dir: PathBuf,
    /// `$XDG_RUNTIME_DIR` when set and the state dir is not overridden.
    pub runtime_dir: Option<PathBuf>,
}

impl Paths {
    /// Resolve from the environment, falling back to platform defaults
    /// (`~/.config/goway`, `~/.local/state/goway`; on Windows
    /// both `%LOCALAPPDATA%\goway`).
    pub fn from_env() -> Self {
        let config_dir =
            std::env::var_os("GOWAY_CONFIG_DIR").map_or_else(default_config_dir, PathBuf::from);
        let state_dir = std::env::var_os("GOWAY_STATE_DIR").map_or_else(
            || {
                dirs::state_dir()
                    .or_else(dirs::data_local_dir)
                    .unwrap_or_else(|| PathBuf::from("."))
                    .join("goway")
            },
            PathBuf::from,
        );
        let runtime_dir = std::env::var_os("XDG_RUNTIME_DIR")
            .filter(|_| std::env::var_os("GOWAY_STATE_DIR").is_none())
            .map(PathBuf::from);
        tracing::debug!(config = %config_dir.display(), state = %state_dir.display(), ?runtime_dir, "paths");
        Self {
            config_dir,
            state_dir,
            runtime_dir,
        }
    }

    /// The config file.
    pub fn config_file(&self) -> PathBuf {
        self.config_dir.join("config.toml")
    }

    /// goway's own `known_hosts`, keyed by `goway-<name>` aliases.
    pub fn known_hosts(&self) -> PathBuf {
        self.config_dir.join("known_hosts")
    }

    /// The cached host state (last good addresses).
    pub fn state_file(&self) -> PathBuf {
        self.state_dir.join("hosts.json")
    }

    /// Directory for ssh `ControlMaster` sockets (Unix clients only):
    /// `$XDG_RUNTIME_DIR/goway` when set (short, private, cleared on
    /// logout), else under the state dir.
    pub fn control_dir(&self) -> PathBuf {
        self.runtime_dir
            .as_ref()
            .map_or_else(|| self.state_dir.join("ssh"), |d| d.join("goway"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_existing_roaming_config_dir_moves_to_the_local_profile() {
        let tmp = tempfile::tempdir().unwrap();
        let legacy = tmp.path().join("Roaming").join("goway");
        let local = tmp.path().join("Local").join("goway");
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::write(legacy.join("id_ed25519"), "key").unwrap();
        assert_eq!(migrate_config_dir(&legacy, &local), local);
        assert_eq!(
            std::fs::read_to_string(local.join("id_ed25519")).unwrap(),
            "key"
        );
        assert!(!legacy.exists());
        // Nothing to move: the local directory is used as is, and an existing one wins.
        assert_eq!(migrate_config_dir(&legacy, &local), local);
        std::fs::create_dir_all(&legacy).unwrap();
        assert_eq!(migrate_config_dir(&legacy, &local), local);
        assert!(
            legacy.exists(),
            "an existing local directory is never merged over"
        );
    }

    #[test]
    fn files_live_under_their_dirs() {
        let p = Paths {
            config_dir: PathBuf::from("/c"),
            state_dir: PathBuf::from("/s"),
            runtime_dir: None,
        };
        assert_eq!(p.config_file(), PathBuf::from("/c/config.toml"));
        assert_eq!(p.known_hosts(), PathBuf::from("/c/known_hosts"));
        assert_eq!(p.state_file(), PathBuf::from("/s/hosts.json"));
        assert_eq!(p.control_dir(), PathBuf::from("/s/ssh"));
        let r = Paths {
            runtime_dir: Some(PathBuf::from("/run/user/1")),
            ..p
        };
        assert_eq!(r.control_dir(), PathBuf::from("/run/user/1/goway"));
    }
}
