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
    /// Holds cached addresses (and control sockets without a runtime dir).
    pub state_dir: PathBuf,
    /// `$XDG_RUNTIME_DIR` when set and the state dir is not overridden.
    pub runtime_dir: Option<PathBuf>,
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
