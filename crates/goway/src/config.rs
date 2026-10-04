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
    /// This machine as a place to run (`[local]`); absent means never.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local: Option<Local>,
}

/// `[local]`: running on this machine. `--host local` always works; the
/// rest is opt-in.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields, default)]
pub struct Local {
    /// Let this machine compete with the helpers when goway picks hosts and shards.
    pub pool: bool,
    /// Run here (with a note) when no helper is reachable.
    pub fallback: bool,
    /// Most goway jobs at once on this machine when it is in the pool.
    pub max_jobs: u32,
    /// Priority of jobs here (default: `defaults.priority`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub priority: Option<Priority>,
    /// Added to this machine's score so helpers win unless it is clearly
    /// less loaded (the machine is also somebody's laptop).
    pub margin: f64,
}

impl Default for Local {
    fn default() -> Self {
        Self {
            pool: false,
            fallback: false,
            max_jobs: 1,
            priority: None,
            margin: 0.5,
        }
    }
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
    /// Most disk goway's remote root may use (`20G`); unset: the smaller of
    /// 20% of the host's disk and 50 GiB. Over it, least recently used
    /// unlocked entries are evicted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_disk: Option<String>,
    /// Free space goway keeps on a host's disk by evicting (`10G`).
    pub min_free: String,
    /// Size cap of each repository's sccache and ccache, unless the user set one (`2G`).
    pub cache_size: String,
    /// Send secret-looking files (env files, credentials, private keys)
    /// to the remote; off: they are never sent.
    #[serde(alias = "send_env_files")]
    pub send_secret_files: bool,
    /// Secret-looking files that may be sent anyway (paths or `*`
    /// patterns, such as `tests/fixtures/*.pem`).
    pub secret_allow: Vec<String>,
    /// The ssh port tried when a host does not set one.
    pub port: u16,
    /// Priority of remote jobs unless a host overrides it.
    pub priority: Priority,
    /// Skip hosts whose 1-minute load per core is above this (unless pinned).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_load: Option<f64>,
    /// GiB of available RAM per core below which a host scores worse (0
    /// turns the penalty off). A 16-core host with 3 GiB runs out of memory
    /// on big builds, so by default it loses to a roomier one.
    pub mem_per_core: f64,
    /// Free RAM one job is assumed to need (`1.5G`): a helper whose available
    /// memory (less what runs still starting will take) is below it gets no
    /// further job, so a wave waits in the queue instead of piling up (`0` turns this off).
    pub job_mem: String,
    /// A helper whose user touched it within this long, or which runs on
    /// battery, counts as in use: it is never skipped for that, but it
    /// scores a little worse and runs jobs extra nicely (`0s` turns this
    /// off). Unknown state (no interop, a Mac without the tool) counts as idle.
    #[serde(with = "humantime_serde")]
    pub owner_idle: Duration,
    /// GPU runs that may share each GPU at once (a run that needs a GPU
    /// holds one GPU slot; 1 gives every GPU run its own GPU).
    pub gpu_jobs: u32,
    /// Extra paths that stay in a build slot's tree between runs, on top of
    /// the detected dependency and build directories (a name matches at
    /// any depth; a path with `/` is relative to the tree root).
    pub keep: Vec<String>,
    /// Also keep every path the tree's `.gitignore` rules ignore (needs git on the host).
    pub keep_ignored: bool,
}

const DAY: u64 = 24 * 60 * 60;

impl Defaults {
    /// The per-job memory reserve in bytes (0 when off or unreadable).
    pub fn job_mem_bytes(&self) -> u64 {
        crate::needs::parse_size(&self.job_mem).unwrap_or(0)
    }

    /// The disk budget words `max_disk:min_free:cache_size` in bytes (max 0 =
    /// automatic), as the remote `run` verb and `gc` take them.
    pub fn budget_bytes(&self) -> (u64, u64, u64) {
        let size = |text: &str| crate::needs::parse_size(text).unwrap_or(0);
        (
            self.max_disk.as_deref().map_or(0, size),
            size(&self.min_free),
            size(&self.cache_size),
        )
    }
}

impl Default for Defaults {
    fn default() -> Self {
        Self {
            remote_root: ".cache/goway".to_owned(),
            cache_ttl: Duration::from_secs(7 * DAY),
            orphan_ttl: Duration::from_secs(DAY),
            kept_ttl: Duration::from_secs(3 * DAY),
            target_slots: 4,
            max_disk: None,
            min_free: "10G".to_owned(),
            cache_size: "2G".to_owned(),
            send_secret_files: false,
            secret_allow: Vec::new(),
            port: 2222,
            priority: Priority::Low,
            max_load: None,
            mem_per_core: crate::pool::DEFAULT_MEM_PER_CORE,
            job_mem: "1.5G".to_owned(),
            owner_idle: Duration::from_mins(5),
            gpu_jobs: 1,
            keep: Vec::new(),
            keep_ignored: true,
        }
    }
}

/// The operating system a host's work runs on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Os {
    /// Linux (including WSL): the shell script side, `remote.sh`.
    #[default]
    Linux,
    /// Windows with PowerShell: the `remote.ps1` side.
    Windows,
}

impl Os {
    /// The word `--needs os=` and the probe use.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Linux => "linux",
            Self::Windows => "windows",
        }
    }
}

/// How goway reaches a host.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Transport {
    /// The system `ssh` to the host (pinned key, `ControlMaster` on Unix).
    #[default]
    Ssh,
    /// The Windows side of this very machine, from WSL, through
    /// `powershell.exe`: no ssh and no network listener.
    Interop,
}

impl Transport {
    /// Whether this is the default (so serializing leaves it out).
    pub fn is_default(&self) -> bool {
        *self == Self::Ssh
    }
}

impl Os {
    /// Whether this is the default (so serializing leaves it out).
    pub fn is_default(&self) -> bool {
        *self == Self::Linux
    }
}

/// One host of the pool. Its identity is `name` plus the pinned host key.
#[derive(Debug, Clone, PartialEq, Default, Deserialize, Serialize)]
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
    /// Free-form labels (`gpu-box`, `fast-disk`) that `--needs label=NAME`
    /// and `goway.toml` rules can ask for.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<String>,
    /// GPU runs that may share each GPU of this host at once (default:
    /// `defaults.gpu_jobs`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpu_jobs: Option<u32>,
    /// Free RAM one job needs on this host (default: `defaults.job_mem`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job_mem: Option<String>,
    /// Most disk goway may use on this host (default: `defaults.max_disk`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_disk: Option<String>,
    /// Free space goway keeps on this host's disk (default: `defaults.min_free`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_free: Option<String>,
    /// Size cap of each repository's compiler caches here (default: `defaults.cache_size`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_size: Option<String>,
    /// The host's operating system (default: linux, which includes WSL).
    #[serde(default, skip_serializing_if = "Os::is_default")]
    pub os: Os,
    /// How goway reaches the host (default: ssh; `interop` is the Windows
    /// side of this machine from WSL, with `os = "windows"`).
    #[serde(default, skip_serializing_if = "Transport::is_default")]
    pub transport: Transport,
}

impl HostConfig {
    /// A host's kind settings must agree: `interop` reaches the Windows
    /// side of this machine, so it needs `os = "windows"` and has no
    /// network address, port, user or key.
    pub fn check_kind(&self) -> std::result::Result<(), &'static str> {
        if self.transport == Transport::Interop {
            if self.os != Os::Windows {
                return Err("uses transport = \"interop\", which needs os = \"windows\"");
            }
            if self.address.is_some()
                || self.port.is_some()
                || self.user.is_some()
                || self.identity.is_some()
            {
                return Err(
                    "uses transport = \"interop\" (no network): drop address, port, user and identity",
                );
            }
        }
        Ok(())
    }

    /// Most goway jobs at once on this host: `max_jobs`, else every core (at
    /// least 1); jobs run niced, so a full helper still yields to its owner.
    pub fn job_limit(&self, cores: u32) -> u32 {
        self.max_jobs.unwrap_or_else(|| cores.max(1))
    }

    /// The ssh `HostKeyAlias` that pins this host's key.
    pub fn key_alias(&self) -> String {
        key_alias(&self.name)
    }
}

/// The ssh `HostKeyAlias` for a host name.
pub fn key_alias(name: &str) -> String {
    format!("goway-{name}")
}

/// `remote_root` is where gc deletes things: it must name a dedicated
/// directory, never the home directory, `/`, or anything outside via `..`.
/// The last component must contain `goway`, so an ordinary directory such as
/// `.ssh` or `Documents` can never be adopted as goway state.
pub fn check_remote_root(root: &str) -> std::result::Result<(), &'static str> {
    let trimmed = root.trim_end_matches('/');
    if trimmed.is_empty() || trimmed == "." || trimmed == "~" {
        return Err("must name a dedicated directory, not the home directory or /");
    }
    if root.chars().any(char::is_control) {
        return Err("must not contain control characters");
    }
    if root.starts_with('~') {
        return Err("is relative to the remote home already; drop the leading ~");
    }
    if trimmed.split('/').any(|c| c == "..") {
        return Err("must not contain `..`");
    }
    let depth = trimmed
        .split('/')
        .filter(|c| !c.is_empty() && *c != ".")
        .count();
    if depth == 0 || (root.starts_with('/') && depth < 2) {
        return Err("is too shallow; use a directory like .cache/goway");
    }
    let last = trimmed.rsplit('/').find(|c| !c.is_empty() && *c != ".");
    if !last.is_some_and(|c| c.to_ascii_lowercase().contains("goway")) {
        return Err(
            "must end in a dedicated directory whose name contains `goway`, like .cache/goway",
        );
    }
    Ok(())
}

/// A `keep` entry is a relative name or glob the remote script matches
/// against a slot tree: never empty, absolute, or climbing out with `..`.
pub fn check_keep_entry(entry: &str) -> std::result::Result<(), &'static str> {
    if entry.trim().is_empty() {
        return Err("is empty");
    }
    if entry.chars().any(char::is_control) {
        return Err("must not contain control characters");
    }
    if entry.starts_with('/') || entry.starts_with('~') {
        return Err("must be relative to the tree root, without a leading / or ~");
    }
    if entry.split('/').any(|c| c == "..") {
        return Err("must not contain `..`");
    }
    Ok(())
}

/// Host names become file and alias components, so keep them plain.
pub fn valid_name(name: &str) -> bool {
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
        if let Err(why) = check_remote_root(&self.defaults.remote_root) {
            return Err(Error::Config {
                path: origin.to_owned(),
                message: format!("remote_root `{}` {why}", self.defaults.remote_root),
            });
        }
        let d = &self.defaults;
        for (key, text) in [
            ("max_disk", d.max_disk.as_deref()),
            ("min_free", Some(d.min_free.as_str())),
            ("cache_size", Some(d.cache_size.as_str())),
        ] {
            if let Some(t) = text
                && crate::needs::parse_size(t).is_none()
            {
                return Err(Error::Config {
                    path: origin.to_owned(),
                    message: format!("{key} `{t}` is not a size such as 20G"),
                });
            }
        }
        for entry in &self.defaults.keep {
            if let Err(why) = check_keep_entry(entry) {
                return Err(Error::Config {
                    path: origin.to_owned(),
                    message: format!("keep entry `{entry}` {why}"),
                });
            }
        }
        for h in &self.hosts {
            for (key, text) in [
                ("max_disk", &h.max_disk),
                ("min_free", &h.min_free),
                ("cache_size", &h.cache_size),
            ] {
                if let Some(t) = text
                    && crate::needs::parse_size(t).is_none()
                {
                    return Err(Error::Config {
                        path: origin.to_owned(),
                        message: format!(
                            "host `{}`: {key} `{t}` is not a size such as 20G",
                            h.name
                        ),
                    });
                }
            }
        }
        if let Some(l) = &self.local {
            if !(1..=1024).contains(&l.max_jobs) {
                return Err(Error::Config {
                    path: origin.to_owned(),
                    message: format!("[local] max_jobs {} must be 1-1024", l.max_jobs),
                });
            }
            if !(l.margin.is_finite() && (0.0..=10.0).contains(&l.margin)) {
                return Err(Error::Config {
                    path: origin.to_owned(),
                    message: format!("[local] margin {} must be between 0 and 10", l.margin),
                });
            }
        }
        if !(1..=64).contains(&self.defaults.gpu_jobs) {
            return Err(Error::Config {
                path: origin.to_owned(),
                message: format!("gpu_jobs {} must be 1-64", self.defaults.gpu_jobs),
            });
        }
        let mut seen = std::collections::BTreeSet::new();
        for host in &self.hosts {
            if host.gpu_jobs.is_some_and(|n| !(1..=64).contains(&n)) {
                return Err(Error::Config {
                    path: origin.to_owned(),
                    message: format!("gpu_jobs of host `{}` must be 1-64", host.name),
                });
            }
            if !valid_name(&host.name) {
                return Err(Error::Config {
                    path: origin.to_owned(),
                    message: format!(
                        "host name `{}` must be 1-63 letters, digits, `-` or `_`",
                        host.name
                    ),
                });
            }
            for label in &host.labels {
                if !valid_name(label) {
                    return Err(Error::Config {
                        path: origin.to_owned(),
                        message: format!(
                            "label `{label}` of host `{}` must be 1-63 letters, digits, `-` or `_`",
                            host.name
                        ),
                    });
                }
            }
            if let Err(why) = host.check_kind() {
                return Err(Error::Config {
                    path: origin.to_owned(),
                    message: format!("host `{}` {why}", host.name),
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

    /// The keep set as the remote `run` verb takes it: the `.gitignore`
    /// switch (`1`/`0`) and the base64 of the newline-joined entries.
    pub fn keep_words(&self) -> (&'static str, String) {
        use base64::Engine as _;
        let ignored = if self.defaults.keep_ignored { "1" } else { "0" };
        let list = self.defaults.keep.join("\n");
        (
            ignored,
            base64::engine::general_purpose::STANDARD.encode(list),
        )
    }

    /// The effective job priority on `host`.
    pub fn priority_of(&self, host: &HostConfig) -> Priority {
        host.priority.unwrap_or(self.defaults.priority)
    }

    /// The per-job memory reserve of `host` in bytes (0: no memory test).
    pub fn job_mem_of(&self, host: &HostConfig) -> u64 {
        host.job_mem.as_deref().map_or_else(
            || self.defaults.job_mem_bytes(),
            |t| crate::needs::parse_size(t).unwrap_or(0),
        )
    }

    /// The disk budget `(max_disk, min_free, cache_size)` in bytes for `host` (or the
    /// defaults when there is no host entry): each value the host sets overrides `[defaults]`.
    pub fn budget_of(&self, host: Option<&HostConfig>) -> (u64, u64, u64) {
        host.map_or_else(
            || self.defaults.budget_bytes(),
            |h| self.for_host(h).defaults.budget_bytes(),
        )
    }

    /// This config with `host`'s disk budget in `[defaults]`, for code that reads the budget
    /// from the defaults (the remote `run` call).
    #[must_use]
    pub fn for_host(&self, host: &HostConfig) -> Self {
        let mut config = self.clone();
        let d = &mut config.defaults;
        d.max_disk = host.max_disk.clone().or_else(|| d.max_disk.take());
        d.min_free = host.min_free.clone().unwrap_or_else(|| d.min_free.clone());
        d.cache_size = host
            .cache_size
            .clone()
            .unwrap_or_else(|| d.cache_size.clone());
        config
    }

    /// The effective load ceiling of `host`.
    pub fn max_load_of(&self, host: &HostConfig) -> Option<f64> {
        host.max_load.or(self.defaults.max_load)
    }

    /// Whether this machine competes with the helpers (`[local] pool = true`)
    /// and is not shadowed by a configured host named `local`.
    pub fn local_in_pool(&self) -> bool {
        self.local.as_ref().is_some_and(|l| l.pool) && self.host(crate::local::NAME).is_err()
    }

    /// Whether goway falls back to this machine when no helper is reachable.
    pub fn local_fallback(&self) -> bool {
        self.local.as_ref().is_some_and(|l| l.fallback) && self.host(crate::local::NAME).is_err()
    }

    /// The effective number of GPU runs per GPU on `host`.
    pub fn gpu_jobs_of(&self, host: &HostConfig) -> u32 {
        host.gpu_jobs.unwrap_or(self.defaults.gpu_jobs)
    }

    /// The effective port of `host`: its own, else Windows OpenSSH's 22 for
    /// a Windows host, else `defaults.port` (the WSL sshd).
    pub fn port_of(&self, host: &HostConfig) -> u16 {
        host.port.unwrap_or(if host.os == Os::Windows {
            22
        } else {
            self.defaults.port
        })
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
    // A file holding only comments keeps them as the document's trailing
    // text, which would end up below the new table: move them above it.
    let leading = doc.trailing().as_str().unwrap_or_default().to_owned();
    let only_comments = doc.as_table().is_empty() && !leading.trim().is_empty();
    if only_comments {
        doc.set_trailing("");
    }
    let mut table = toml_edit::ser::to_document(host)
        .map_err(|e| Error::Config {
            path: path.to_owned(),
            message: e.to_string(),
        })?
        .as_table()
        .clone();
    if only_comments {
        table.decor_mut().set_prefix(leading);
    }
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

/// Write via a fresh temp file and rename, so readers never see half a
/// file; flushed to disk before the rename. A new file is owner-only
/// (0600) on Unix; an existing file keeps its mode.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write as _;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| Error::io("create", dir, e))?;
    }
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    let tmp = path.with_extension(format!("tmp.{}.{nanos}", std::process::id()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    // An existing file keeps its mode (edits and undo leave it as it was).
    let existing = std::fs::metadata(path).ok().map(|m| m.permissions());
    let written = options.open(&tmp).and_then(|mut f| {
        f.write_all(bytes)?;
        if let Some(perms) = existing {
            f.set_permissions(perms)?;
        }
        f.sync_all()
    });
    if let Err(e) = written {
        let _ = std::fs::remove_file(&tmp);
        return Err(Error::io("write", &tmp, e));
    }
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

    // frob:ticket 01M43JBGHCD68QDZVMDMG5P4GS
    // frob:tests crates/goway/src/config.rs::Config::budget_of
    #[test]
    fn a_hosts_disk_budget_overrides_the_defaults_value_by_value() {
        let c = Config::parse(
            "[defaults]\nmax_disk = \"300G\"\nmin_free = \"10G\"\n\n\
             [[host]]\nname = \"small\"\nmax_disk = \"30G\"\ncache_size = \"1G\"\n\n\
             [[host]]\nname = \"big\"\n",
            Path::new("c.toml"),
        )
        .unwrap();
        assert_eq!(
            c.budget_of(c.host("small").ok()),
            (30 << 30, 10 << 30, 1 << 30)
        );
        assert_eq!(
            c.budget_of(c.host("big").ok()),
            (300 << 30, 10 << 30, 2 << 30)
        );
        assert_eq!(c.budget_of(None), (300 << 30, 10 << 30, 2 << 30));
        // The remote run call reads the same values from the host's view of the config.
        assert_eq!(
            c.for_host(c.host("small").unwrap()).defaults.budget_bytes(),
            (30 << 30, 10 << 30, 1 << 30)
        );
        // A bad size on a host is a config error naming the host.
        let err = Config::parse(
            "[[host]]\nname = \"x\"\nmin_free = \"lots\"\n",
            Path::new("c.toml"),
        )
        .unwrap_err()
        .to_string();
        assert!(
            err.contains("host `x`") && err.contains("min_free"),
            "{err}"
        );
    }

    #[test]
    fn disk_budget_defaults_and_validation() {
        let c = Config::default();
        assert_eq!(c.defaults.budget_bytes(), (0, 10 << 30, 2 << 30));
        let c = Config::parse(
            "[defaults]\nmax_disk = \"30G\"\nmin_free = \"5G\"\ncache_size = \"512M\"\n",
            Path::new("c.toml"),
        )
        .unwrap();
        assert_eq!(c.defaults.budget_bytes(), (30 << 30, 5 << 30, 512 << 20));
        assert!(Config::parse("[defaults]\nmax_disk = \"lots\"\n", Path::new("c.toml")).is_err());
    }

    #[test]
    fn keep_entries_are_validated_and_encoded() {
        for bad in ["", "/abs", "~/x", "a/../b", "a\nb"] {
            assert!(check_keep_entry(bad).is_err(), "{bad:?}");
        }
        let c = Config::parse(
            "[defaults]\nkeep = [\"node_modules\", \"out/cache\"]\nkeep_ignored = false\n",
            Path::new("c.toml"),
        )
        .unwrap();
        let (ignored, b64) = c.keep_words();
        assert_eq!(ignored, "0");
        assert!(!b64.is_empty());
        assert!(Config::parse("[defaults]\nkeep = [\"../x\"]\n", Path::new("c.toml")).is_err());
        assert_eq!(Config::default().keep_words().0, "1");
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
    fn remote_root_must_be_a_dedicated_directory() {
        for bad in [
            "",
            ".",
            "./",
            "/",
            "~",
            "~/x",
            "a/../..",
            "/tmp",
            "x\ny",
            ".ssh",
            "Documents",
            "/home/u/projects",
        ] {
            assert!(check_remote_root(bad).is_err(), "{bad:?}");
        }
        for good in [".cache/goway", "/srv/goway", "work/goway-state"] {
            assert!(check_remote_root(good).is_ok(), "{good:?}");
        }
        let e =
            Config::parse("[defaults]\nremote_root = \".\"\n", Path::new("c.toml")).unwrap_err();
        assert!(e.to_string().contains("remote_root"), "{e}");
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
            port: Some(2222),
            ..HostConfig::default()
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
    fn add_host_keeps_a_leading_comment_on_top() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "# my pool\n").unwrap();
        let host = HostConfig {
            name: "q".to_owned(),
            ..HostConfig::default()
        };
        add_host(&path, &host).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("# my pool\n"), "{text}");
        assert!(text.contains("[[host]]\nname = \"q\""), "{text}");
    }

    #[cfg(unix)]
    #[test]
    fn writes_are_private_and_leave_no_temp_files() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("known_hosts");
        write_atomic(&path, b"one").unwrap();
        write_atomic(&path, b"two").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "two");
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
        let shared = dir.path().join("config.toml");
        std::fs::write(&shared, "a").unwrap();
        std::fs::set_permissions(&shared, std::fs::Permissions::from_mode(0o644)).unwrap();
        write_atomic(&shared, b"b").unwrap();
        assert_eq!(
            std::fs::metadata(&shared).unwrap().permissions().mode() & 0o777,
            0o644
        );
    }

    // frob:tests crates/goway/src/config.rs::job_limit
    #[test]
    fn job_limit_defaults_to_all_the_cores_and_honours_max_jobs() {
        let mut host: HostConfig = toml::from_str("name = \"h\"").unwrap();
        assert_eq!(host.job_limit(16), 16);
        assert_eq!(host.job_limit(3), 3);
        assert_eq!(host.job_limit(0), 1);
        host.max_jobs = Some(5);
        assert_eq!(host.job_limit(16), 5);
    }

    #[test]
    fn add_host_creates_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub/config.toml");
        let host = HostConfig {
            name: "q".to_owned(),
            address: Some("10.0.0.1".to_owned()),
            max_jobs: Some(1),
            ..HostConfig::default()
        };
        add_host(&path, &host).unwrap();
        assert_eq!(Config::load(&path).unwrap().hosts, vec![host]);
        assert_eq!(
            Config::load(&dir.path().join("none.toml")).unwrap(),
            Config::default()
        );
    }
    // frob:tests crates/goway/src/config.rs::Local
    #[test]
    fn the_local_section_is_opt_in_validated_and_closed() {
        let origin = Path::new("config.toml");
        assert!(Config::parse("", origin).unwrap().local.is_none());
        let c = Config::parse("[local]\n", origin).unwrap();
        assert!(
            !c.local_in_pool() && !c.local_fallback(),
            "a bare section opts into nothing"
        );
        let c = Config::parse(
            "[local]\npool = true\nfallback = true\nmax_jobs = 2\nmargin = 0.25\npriority = \"normal\"\n",
            origin,
        )
        .unwrap();
        let l = c.local.as_ref().unwrap();
        assert_eq!((l.max_jobs, l.priority), (2, Some(Priority::Normal)));
        assert!(c.local_in_pool() && c.local_fallback());
        for bad in [
            "[local]\nbogus = 1\n",
            "[local]\nmax_jobs = 0\n",
            "[local]\nmargin = 11\n",
            "[local]\nmargin = -1\n",
            "[local]\nmargin = nan\n",
        ] {
            assert!(Config::parse(bad, origin).is_err(), "{bad}");
        }
        // A configured host called local wins over this machine.
        let c = Config::parse(
            "[local]\npool = true\n\n[[host]]\nname = \"local\"\n",
            origin,
        )
        .unwrap();
        assert!(!c.local_in_pool());
    }
}
