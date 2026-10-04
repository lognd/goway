//! Cached host state: the last address and port that passed the host key
//! check. Only a hint for the next run; losing it costs one re-resolution.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::config::write_atomic;
use crate::error::{Error, Result};
use crate::facts::Cached;

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

/// Programs of one repository whose shard detection failed.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct ShardMarks {
    /// The repository's readable name (so `gc --repo NAME` finds it).
    #[serde(default)]
    pub repo: String,
    /// The programs as typed on the command line.
    #[serde(default)]
    pub programs: Vec<String>,
}

/// Days without a copy mismatch after which a distrusted repository is trusted again.
pub const DISTRUST_DAYS: u64 = 7;

/// A repository on a host whose copy once failed verification: it gets
/// full verification and a fresh slot every run until it has been clean
/// for [`DISTRUST_DAYS`] (kept only in local state, never sent anywhere).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct Distrust {
    /// The host's name.
    #[serde(default)]
    pub host: String,
    /// The repository's readable name (so `gc --repo NAME` finds it).
    #[serde(default)]
    pub repo: String,
    /// Seconds since the epoch of the last proven mismatch.
    #[serde(default)]
    pub last_mismatch: u64,
}

/// The tool versions `goway doctor` last saw on a host, kept so a run can
/// note fleet drift and record what was used without probing again.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct ToolVersions {
    /// When doctor captured them, in seconds since the Unix epoch.
    #[serde(default)]
    pub captured: u64,
    /// Tool name to the first line of its version report.
    #[serde(default)]
    pub versions: BTreeMap<String, String>,
}

/// The state file: host name (lowercase) to its cached state.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct State {
    /// Cached state per host.
    #[serde(default)]
    pub hosts: BTreeMap<String, HostState>,
    /// Cached static host facts (GPUs, CPU features, KVM, Docker), per host.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub facts: BTreeMap<String, Cached>,
    /// Programs whose test-framework detection failed, per repository id
    /// (kept only here, never sent to a host; cleared by `goway gc --repo`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub shard_marks: BTreeMap<String, ShardMarks>,
    /// Repositories on hosts whose copy failed verification, by `host/repo-id`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub distrust: BTreeMap<String, Distrust>,
    /// Tool versions doctor last saw, per host (lowercase name).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub tool_versions: BTreeMap<String, ToolVersions>,
    /// Probe static facts on the next probe regardless of age (not saved).
    #[serde(skip)]
    pub refresh_facts: bool,
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

    /// Whether `repo_id` on `host` is distrusted at `now` (a mark older than
    /// [`DISTRUST_DAYS`] no longer counts).
    pub fn distrusted(&self, host: &str, repo_id: &str, now: u64) -> bool {
        self.distrust
            .get(&distrust_key(host, repo_id))
            .is_some_and(|d| now.saturating_sub(d.last_mismatch) < DISTRUST_DAYS * 86_400)
    }

    /// Record a proven copy mismatch of `repo_id` (named `repo`) on `host`.
    pub fn mark_mismatch(&mut self, host: &str, repo: &str, repo_id: &str, now: u64) {
        tracing::warn!(
            host,
            repo,
            "remembering a copy mismatch: full verification from now on"
        );
        self.distrust.insert(
            distrust_key(host, repo_id),
            Distrust {
                host: host.to_owned(),
                repo: repo.to_owned(),
                last_mismatch: now,
            },
        );
    }

    /// Forget the distrust of the repository named or identified by `filter`
    /// (all of them with no filter) and any that expired by `now`; returns
    /// how many were forgotten.
    pub fn clear_distrust(&mut self, filter: Option<&str>, now: u64) -> usize {
        let before = self.distrust.len();
        self.distrust.retain(|key, d| {
            let expired = now.saturating_sub(d.last_mismatch) >= DISTRUST_DAYS * 86_400;
            !expired && filter.is_some_and(|f| key.rsplit('/').next() != Some(f) && d.repo != f)
        });
        before - self.distrust.len()
    }

    /// Forget `name` (when a host is removed).
    pub fn forget(&mut self, name: &str) {
        self.hosts.remove(&name.to_ascii_lowercase());
        self.facts.remove(&name.to_ascii_lowercase());
        self.tool_versions.remove(&name.to_ascii_lowercase());
    }

    /// Remember the tool versions doctor saw on `host` at `now`.
    pub fn record_tools(&mut self, host: &str, now: u64, versions: BTreeMap<String, String>) {
        self.tool_versions.insert(
            host.to_ascii_lowercase(),
            ToolVersions {
                captured: now,
                versions,
            },
        );
    }
}

fn distrust_key(host: &str, repo_id: &str) -> String {
    format!("{}/{repo_id}", host.to_ascii_lowercase())
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

    #[test]
    fn distrust_expires_after_seven_clean_days_and_clears_by_repo() {
        let mut s = State::default();
        s.mark_mismatch("Helios", "proj", "id1", 1_000);
        assert!(s.distrusted("helios", "id1", 1_000 + 6 * 86_400));
        assert!(!s.distrusted("helios", "id1", 1_000 + 7 * 86_400));
        assert!(!s.distrusted("other", "id1", 1_000));
        s.mark_mismatch("helios", "proj", "id1", 2_000);
        s.mark_mismatch("orion", "proj2", "id2", 2_000);
        assert_eq!(s.clear_distrust(Some("proj"), 3_000), 1);
        assert!(s.distrusted("orion", "id2", 3_000));
        assert_eq!(s.clear_distrust(Some("id2"), 3_000), 1);
        assert!(s.distrust.is_empty());
    }

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
