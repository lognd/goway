//! Cached host state: the last address and port that passed the host key
//! check. Only a hint for the next run; losing it costs one re-resolution.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::config::write_atomic;
use crate::error::{Error, Result};

/// What goway remembers about one host.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct HostState {
    /// The last address that worked.
    pub address: String,
    /// The port it worked on.
    pub port: u16,
    /// When it last worked, in seconds since the Unix epoch.
    pub last_ok: u64,
}

/// The state file: host name (lowercase) to its cached state.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct State {
    /// Cached state per host.
    #[serde(default)]
    pub hosts: BTreeMap<String, HostState>,
}

impl State {
    /// Load from `path`; a missing file is empty state.
    pub fn load(path: &Path) -> Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(text) => serde_json::from_str(&text).map_err(|e| Error::State {
                path: path.to_owned(),
                message: e.to_string(),
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(Error::io("read", path, e)),
        }
    }

    /// Save atomically to `path`.
    pub fn save(&self, path: &Path) -> Result<()> {
        let text = serde_json::to_string_pretty(self).map_err(|e| Error::State {
            path: path.to_owned(),
            message: e.to_string(),
        })?;
        write_atomic(path, text.as_bytes())?;
        tracing::debug!(path = %path.display(), hosts = self.hosts.len(), "state saved");
        Ok(())
    }

    /// The cached state of `name`.
    pub fn get(&self, name: &str) -> Option<&HostState> {
        self.hosts.get(&name.to_ascii_lowercase())
    }

    /// Record that `address:port` just worked for `name`.
    pub fn remember(&mut self, name: &str, address: &str, port: u16, now: u64) {
        tracing::info!(host = name, address, port, "remembering working address");
        self.hosts.insert(
            name.to_ascii_lowercase(),
            HostState {
                address: address.to_owned(),
                port,
                last_ok: now,
            },
        );
    }

    /// Forget `name` (when a host is removed).
    pub fn forget(&mut self, name: &str) {
        self.hosts.remove(&name.to_ascii_lowercase());
    }
}

/// Seconds since the Unix epoch.
pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    // frob:tests crates/goway/src/state.rs::now_secs
    #[test]
    fn remember_save_load_forget() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s/hosts.json");
        assert_eq!(State::load(&path).unwrap(), State::default());
        let mut s = State::default();
        let now = now_secs();
        s.remember("Helios", "192.0.2.10", 2222, now);
        s.save(&path).unwrap();
        let loaded = State::load(&path).unwrap();
        assert_eq!(loaded.get("helios").unwrap().address, "192.0.2.10");
        assert_eq!(loaded.get("HELIOS").unwrap().last_ok, now);
        let mut loaded = loaded;
        loaded.forget("helios");
        assert!(loaded.get("helios").is_none());
    }

    #[test]
    fn corrupt_state_is_a_typed_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hosts.json");
        std::fs::write(&path, "{nope").unwrap();
        assert!(matches!(State::load(&path), Err(Error::State { .. })));
    }
}
