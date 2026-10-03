//! Where goway keeps its local files: config, state and pinned host keys.
//!
//! `GOWAY_CONFIG_DIR` and `GOWAY_STATE_DIR` override the platform defaults
//! (tests rely on them; users can too).

use std::path::PathBuf;

/// The local directories goway uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    /// Holds `config.toml` and `known_hosts`.
    pub config_dir: PathBuf,
    /// Holds cached addresses and ssh control sockets.
    pub state_dir: PathBuf,
}

impl Paths {
    /// Resolve from the environment, falling back to platform defaults
    /// (`~/.config/goway`, `~/.local/state/goway`; on Windows
    /// `%APPDATA%\goway` and `%LOCALAPPDATA%\goway`).
    pub fn from_env() -> Self {
        let config_dir = std::env::var_os("GOWAY_CONFIG_DIR").map_or_else(
            || {
                dirs::config_dir()
                    .unwrap_or_else(|| PathBuf::from("."))
                    .join("goway")
            },
            PathBuf::from,
        );
        let state_dir = std::env::var_os("GOWAY_STATE_DIR").map_or_else(
            || {
                dirs::state_dir()
                    .or_else(dirs::data_local_dir)
                    .unwrap_or_else(|| PathBuf::from("."))
                    .join("goway")
            },
            PathBuf::from,
        );
        tracing::debug!(config = %config_dir.display(), state = %state_dir.display(), "paths");
        Self {
            config_dir,
            state_dir,
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

    /// Directory for ssh `ControlMaster` sockets (Unix clients only).
    pub fn control_dir(&self) -> PathBuf {
        self.state_dir.join("ssh")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_live_under_their_dirs() {
        let p = Paths {
            config_dir: PathBuf::from("/c"),
            state_dir: PathBuf::from("/s"),
        };
        assert_eq!(p.config_file(), PathBuf::from("/c/config.toml"));
        assert_eq!(p.known_hosts(), PathBuf::from("/c/known_hosts"));
        assert_eq!(p.state_file(), PathBuf::from("/s/hosts.json"));
        assert_eq!(p.control_dir(), PathBuf::from("/s/ssh"));
    }
}
