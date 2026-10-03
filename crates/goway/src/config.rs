//! The TOML config file: defaults and the host pool.
//!
//! Unknown keys are rejected so a typo never silently changes behaviour.
//! Writes go through `toml_edit` so the user's comments and layout survive.

use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// The whole config file.
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Settings shared by all hosts.
    #[serde(default)]
    pub defaults: Defaults,
    /// The host pool, in file order.
    #[serde(default, rename = "host")]
    pub hosts: Vec<HostConfig>,
}

/// CPU and I/O priority of remote jobs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Priority {
    /// `nice -n 10` and idle-class I/O: the host's own user comes first.
    #[default]
    Low,
    /// The remote user's normal priority.
    Normal,
}

impl Priority {
    /// The word the remote script understands.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Normal => "normal",
        }
    }
}

/// Settings shared by all hosts.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields, default)]
pub struct Defaults {
    /// Root of goway's remote state, relative to the remote home if not absolute.
    pub remote_root: String,
    /// Idle time after which seeds and caches expire.
    #[serde(with = "humantime_serde")]
    pub cache_ttl: Duration,
    /// Age after which an unlocked, not kept work directory is removed.
    #[serde(with = "humantime_serde")]
    pub orphan_ttl: Duration,
    /// Age after which a `--keep` work directory is removed.
    #[serde(with = "humantime_serde")]
    pub kept_ttl: Duration,
    /// Most cargo target directories per repository on one host.
    pub target_slots: u32,
    /// Send `.env` files to the remote (off: they are never sent).
    pub send_env_files: bool,
    /// The ssh port tried when a host does not set one.
    pub port: u16,
    /// Priority of remote jobs unless a host overrides it.
    pub priority: Priority,
    /// Skip hosts whose 1-minute load per core is above this (unless pinned).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_load: Option<f64>,
}

const DAY: u64 = 24 * 60 * 60;

impl Default for Defaults {
    fn default() -> Self {
        Self {
            remote_root: ".cache/goway".to_owned(),
            cache_ttl: Duration::from_secs(7 * DAY),
            orphan_ttl: Duration::from_secs(DAY),
            kept_ttl: Duration::from_secs(3 * DAY),
            target_slots: 4,
            send_env_files: false,
            port: 2222,
            priority: Priority::Low,
            max_load: None,
        }
    }
}

/// One host of the pool. Its identity is `name` plus the pinned host key.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HostConfig {
    /// Identity and ssh `HostKeyAlias` (`goway-<name>`); also tried as `<name>.local`.
    pub name: String,
    /// An address to try (DNS name or IP); never required to stay valid.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
    /// The ssh port (default: `defaults.port`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    /// The remote user (default: from ssh config).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    /// Most goway jobs at once on this host (default: unlimited).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_jobs: Option<u32>,
    /// Priority of jobs here (default: `defaults.priority`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<Priority>,
    /// Skip this host above this load per core (default: `defaults.max_load`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_load: Option<f64>,
    /// Private key to offer (set by `goway ssh setup` when it created
    /// goway's own key); default: whatever ssh would use.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<String>,
}

impl HostConfig {
    /// The ssh `HostKeyAlias` that pins this host's key.
    pub fn key_alias(&self) -> String {
        key_alias(&self.name)
    }
}

/// The ssh `HostKeyAlias` for a host name.
pub fn key_alias(name: &str) -> String {
    format!("goway-{name}")
}

/// Host names become file and alias components, so keep them plain.
fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 63
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        && !name.starts_with('-')
}

impl Config {
    /// Parse and validate config text; `origin` names it in errors.
    pub fn parse(text: &str, origin: &Path) -> Result<Self> {
        let config: Self = toml::from_str(text).map_err(|e| Error::Config {
            path: origin.to_owned(),
            message: e.to_string(),
        })?;
        config.validate(origin)?;
        Ok(config)
    }

    /// Load from `path`; a missing file is an empty config.
    pub fn load(path: &Path) -> Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(text) => {
                let config = Self::parse(&text, path)?;
                tracing::debug!(path = %path.display(), hosts = config.hosts.len(), "config loaded");
                Ok(config)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                tracing::debug!(path = %path.display(), "no config file; using defaults");
                Ok(Self::default())
            }
            Err(e) => Err(Error::io("read", path, e)),
        }
    }

    fn validate(&self, origin: &Path) -> Result<()> {
        let mut seen = std::collections::BTreeSet::new();
        for host in &self.hosts {
            if !valid_name(&host.name) {
                return Err(Error::Config {
                    path: origin.to_owned(),
                    message: format!(
                        "host name `{}` must be 1-63 letters, digits, `-` or `_`",
                        host.name
                    ),
                });
            }
            if !seen.insert(host.name.to_ascii_lowercase()) {
                return Err(Error::Config {
                    path: origin.to_owned(),
                    message: format!("host `{}` is listed twice", host.name),
                });
            }
        }
        Ok(())
    }

    /// The host named `name` (case-insensitive).
    pub fn host(&self, name: &str) -> Result<&HostConfig> {
        self.hosts
            .iter()
            .find(|h| h.name.eq_ignore_ascii_case(name))
            .ok_or_else(|| Error::UnknownHost(name.to_owned()))
    }

    /// The effective job priority on `host`.
    pub fn priority_of(&self, host: &HostConfig) -> Priority {
        host.priority.unwrap_or(self.defaults.priority)
    }

    /// The effective load ceiling of `host`.
    pub fn max_load_of(&self, host: &HostConfig) -> Option<f64> {
        host.max_load.or(self.defaults.max_load)
    }

    /// The effective port of `host`.
    pub fn port_of(&self, host: &HostConfig) -> u16 {
        host.port.unwrap_or(self.defaults.port)
    }
}

/// Append a `[[host]]` table to the config file, keeping everything else intact.
pub fn add_host(path: &Path, host: &HostConfig) -> Result<()> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(Error::io("read", path, e)),
    };
    let existing = Config::parse(&text, path)?;
    if existing.host(&host.name).is_ok() {
        return Err(Error::Config {
            path: path.to_owned(),
            message: format!("host `{}` already exists", host.name),
        });
    }
    let mut doc: toml_edit::DocumentMut =
        text.parse()
            .map_err(|e: toml_edit::TomlError| Error::Config {
                path: path.to_owned(),
                message: e.to_string(),
            })?;
    let table = toml_edit::ser::to_document(host)
        .map_err(|e| Error::Config {
            path: path.to_owned(),
            message: e.to_string(),
        })?
        .as_table()
        .clone();
    let hosts = doc
        .entry("host")
        .or_insert_with(|| toml_edit::Item::ArrayOfTables(toml_edit::ArrayOfTables::new()));
    let Some(array) = hosts.as_array_of_tables_mut() else {
        return Err(Error::Config {
            path: path.to_owned(),
            message: "`host` must be an array of tables ([[host]])".to_owned(),
        });
    };
    array.push(table);
    let out = doc.to_string();
    Config::parse(&out, path)?;
    write_atomic(path, out.as_bytes())?;
    tracing::info!(host = %host.name, path = %path.display(), "host added to config");
    Ok(())
}

/// Set or clear the `identity` of host `name`; other text is untouched.
pub fn set_host_identity(path: &Path, name: &str, identity: Option<&str>) -> Result<()> {
    let text = std::fs::read_to_string(path).map_err(|e| Error::io("read", path, e))?;
    Config::parse(&text, path)?.host(name)?;
    let mut doc: toml_edit::DocumentMut =
        text.parse()
            .map_err(|e: toml_edit::TomlError| Error::Config {
                path: path.to_owned(),
                message: e.to_string(),
            })?;
    if let Some(array) = doc
        .get_mut("host")
        .and_then(toml_edit::Item::as_array_of_tables_mut)
    {
        for table in array.iter_mut() {
            let matches = table
                .get("name")
                .and_then(toml_edit::Item::as_str)
                .is_some_and(|n| n.eq_ignore_ascii_case(name));
            if matches {
                match identity {
                    Some(i) => {
                        table.insert("identity", toml_edit::value(i));
                    }
                    None => {
                        table.remove("identity");
                    }
                }
            }
        }
    }
    let out = doc.to_string();
    Config::parse(&out, path)?;
    write_atomic(path, out.as_bytes())?;
    tracing::info!(host = name, ?identity, "host identity updated");
    Ok(())
}

/// Remove the `[[host]]` table named `name`; other text is untouched.
pub fn remove_host(path: &Path, name: &str) -> Result<()> {
    let text = std::fs::read_to_string(path).map_err(|e| Error::io("read", path, e))?;
    Config::parse(&text, path)?.host(name)?;
    let mut doc: toml_edit::DocumentMut =
        text.parse()
            .map_err(|e: toml_edit::TomlError| Error::Config {
                path: path.to_owned(),
                message: e.to_string(),
            })?;
    if let Some(array) = doc
        .get_mut("host")
        .and_then(toml_edit::Item::as_array_of_tables_mut)
    {
        array.retain(|t| {
            !t.get("name")
                .and_then(toml_edit::Item::as_str)
                .is_some_and(|n| n.eq_ignore_ascii_case(name))
        });
    }
    write_atomic(path, doc.to_string().as_bytes())?;
    tracing::info!(host = name, path = %path.display(), "host removed from config");
    Ok(())
}

/// Write via a temp file and rename so readers never see half a file.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| Error::io("create", dir, e))?;
    }
    let tmp = path.with_extension(format!("tmp.{}", std::process::id()));
    std::fs::write(&tmp, bytes).map_err(|e| Error::io("write", &tmp, e))?;
    std::fs::rename(&tmp, path).map_err(|e| Error::io("rename", path, e))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
# my pool
[defaults]
cache_ttl = "5d"
orphan_ttl = "12h"

[[host]]
name = "helios"
address = "Helios"
max_jobs = 2

[[host]]
name = "orion-notebook"
port = 22
user = "user"
"#;

    #[test]
    fn parses_hosts_defaults_and_durations() {
        let c = Config::parse(SAMPLE, Path::new("c.toml")).unwrap();
        assert_eq!(c.defaults.cache_ttl, Duration::from_secs(5 * DAY));
        assert_eq!(c.defaults.orphan_ttl, Duration::from_hours(12));
        assert_eq!(c.defaults.kept_ttl, Duration::from_secs(3 * DAY));
        assert_eq!(c.defaults.target_slots, 4);
        assert_eq!(c.hosts.len(), 2);
        let q = c.host("HELIOS").unwrap();
        assert_eq!(q.address.as_deref(), Some("Helios"));
        assert_eq!(c.port_of(q), 2222);
        assert_eq!(c.port_of(c.host("orion-notebook").unwrap()), 22);
        assert_eq!(q.key_alias(), "goway-helios");
    }

    #[test]
    fn rejects_unknown_keys() {
        for text in [
            "[defaults]\ncache_tll = \"1d\"\n",
            "[[host]]\nname = \"a\"\nadress = \"x\"\n",
            "hosts = []\n",
        ] {
            let e = Config::parse(text, Path::new("c.toml")).unwrap_err();
            assert!(e.to_string().contains("unknown field"), "{e}");
        }
    }

    #[test]
    fn rejects_bad_and_duplicate_names() {
        for text in [
            "[[host]]\nname = \"a b\"\n",
            "[[host]]\nname = \"a\"\n[[host]]\nname = \"A\"\n",
        ] {
            assert!(Config::parse(text, Path::new("c.toml")).is_err(), "{text}");
        }
    }

    #[test]
    fn unknown_host_is_typed() {
        let c = Config::default();
        assert!(matches!(c.host("x"), Err(Error::UnknownHost(n)) if n == "x"));
    }

    // frob:tests crates/goway/src/config.rs::write_atomic
    #[test]
    fn add_and_remove_host_keep_comments() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, SAMPLE).unwrap();
        let host = HostConfig {
            name: "nova".to_owned(),
            address: None,
            port: Some(2222),
            user: None,
            max_jobs: None,
            priority: None,
            max_load: None,
            identity: None,
        };
        add_host(&path, &host).unwrap();
        assert!(add_host(&path, &host).is_err());
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("# my pool"));
        let c = Config::load(&path).unwrap();
        assert_eq!(c.host("nova").unwrap(), &host);
        remove_host(&path, "helios").unwrap();
        let c = Config::load(&path).unwrap();
        assert!(c.host("helios").is_err());
        assert_eq!(c.hosts.len(), 2);
        assert!(
            std::fs::read_to_string(&path)
                .unwrap()
                .contains("# my pool")
        );
    }

    #[test]
    fn add_host_creates_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub/config.toml");
        let host = HostConfig {
            name: "q".to_owned(),
            address: Some("10.0.0.1".to_owned()),
            port: None,
            user: None,
            max_jobs: Some(1),
            priority: None,
            max_load: None,
            identity: None,
        };
        add_host(&path, &host).unwrap();
        assert_eq!(Config::load(&path).unwrap().hosts, vec![host]);
        assert_eq!(
            Config::load(&dir.path().join("none.toml")).unwrap(),
            Config::default()
        );
    }
}
