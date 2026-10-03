//! `goway host add`: find a new host's Linux sshd and pin its key.
//!
//! Pinning is trust on first use, so goway is careful about *which* machine
//! it trusts: candidates are probed against a scratch `known_hosts`, the
//! machine must be Linux, and its hostname must match the host name (or the
//! user must have named the address explicitly). Only then is the key
//! copied into goway's `known_hosts` under `goway-<name>`.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::cli::HostAddArgs;
use crate::config::{self, Config, HostConfig};
use crate::error::{Error, Result};
use crate::paths::Paths;
use crate::render::Renderer;
use crate::resolve::{self, Lookup, ProbeResult, Prober, SshProber};
use crate::ssh::{self, Failure, KeyPolicy, Target};
use crate::sshenv;
use crate::state::State;

/// The probe `host add` runs: OS, hostname, arch, one per line.
pub const IDENTIFY: &str = "uname -s; uname -n; uname -m";

/// What a probed machine says about itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    /// `uname -s`.
    pub os: String,
    /// `uname -n`.
    pub hostname: String,
    /// `uname -m`.
    pub arch: String,
}

/// Parse the [`IDENTIFY`] output.
pub fn parse_identity(text: &str) -> Option<Identity> {
    let mut lines = text.lines().map(str::trim);
    Some(Identity {
        os: lines.next()?.to_owned(),
        hostname: lines.next()?.to_owned(),
        arch: lines.next()?.to_owned(),
    })
}

/// Whether `hostname` is the machine the user means by `name` / `address`.
pub fn hostname_matches(hostname: &str, name: &str, address: Option<&str>) -> bool {
    let short = |s: &str| s.split('.').next().unwrap_or(s).to_ascii_lowercase();
    let h = short(hostname);
    h == short(name) || address.is_some_and(|a| h == short(a))
}

/// Probes with a scratch `known_hosts`, accepting a candidate only if it is
/// the intended Linux machine; rejected keys are removed before the next try.
struct AddProber<'a> {
    inner: SshProber,
    name: &'a str,
    address: Option<&'a str>,
    trust_address: bool,
}

impl Prober for AddProber<'_> {
    fn probe(&self, target: &Target, policy: KeyPolicy, remote: &str) -> ProbeResult {
        let out = self.inner.probe(target, policy, remote);
        let reject = |why: String| {
            forget_key(
                &self.inner.settings.known_hosts,
                &config::key_alias(self.name),
            );
            Err((Failure::Other, why))
        };
        match out {
            Ok(text) => match parse_identity(&text) {
                Some(id) if id.os != "Linux" => {
                    reject(format!("{} is {}, not Linux", target.address, id.os))
                }
                Some(id)
                    if !self.trust_address
                        && !hostname_matches(&id.hostname, self.name, self.address) =>
                {
                    reject(format!(
                        "{} is `{}`, not `{}` (pass --address with its IP to trust it anyway)",
                        target.address, id.hostname, self.name
                    ))
                }
                Some(_) => Ok(text),
                None => reject(format!("{} gave no identity", target.address)),
            },
            Err((failure, stderr)) => {
                forget_key(
                    &self.inner.settings.known_hosts,
                    &config::key_alias(self.name),
                );
                // A Windows sshd runs `uname` in cmd.exe and fails with 1.
                let failure = if failure == Failure::Other && stderr.contains("not recognized") {
                    tracing::info!(address = %target.address, port = target.port, "Windows sshd answered");
                    Failure::Other
                } else {
                    failure
                };
                Err((failure, stderr))
            }
        }
    }
}

/// Remove `alias` from a `known_hosts` file (no-op if absent).
fn forget_key(known_hosts: &Path, alias: &str) {
    if !known_hosts.exists() {
        return;
    }
    let result = Command::new("ssh-keygen")
        .arg("-R")
        .arg(alias)
        .arg("-f")
        .arg(known_hosts)
        .output();
    match result {
        Ok(out) if out.status.success() => {
            tracing::debug!(alias, file = %known_hosts.display(), "key forgotten");
        }
        Ok(out) => {
            tracing::warn!(alias, stderr = %String::from_utf8_lossy(&out.stderr), "ssh-keygen -R failed");
        }
        Err(e) => tracing::warn!(alias, error = %e, "cannot run ssh-keygen"),
    }
    let old = known_hosts.with_extension("old");
    let _ = std::fs::remove_file(old);
}

/// The SHA256 fingerprint line(s) of a `known_hosts` file.
fn fingerprint(known_hosts: &Path) -> String {
    Command::new("ssh-keygen")
        .arg("-l")
        .arg("-f")
        .arg(known_hosts)
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .unwrap_or_default()
}

/// Move the pinned entry from the scratch file into goway's `known_hosts`.
fn adopt_key(scratch: &Path, known_hosts: &Path, alias: &str) -> Result<()> {
    let entry = std::fs::read_to_string(scratch).map_err(|e| Error::io("read", scratch, e))?;
    forget_key(known_hosts, alias);
    let mut current = match std::fs::read_to_string(known_hosts) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(Error::io("read", known_hosts, e)),
    };
    if !current.is_empty() && !current.ends_with('\n') {
        current.push('\n');
    }
    current.push_str(&entry);
    config::write_atomic(known_hosts, current.as_bytes())?;
    tracing::info!(alias, file = %known_hosts.display(), "host key pinned");
    Ok(())
}

/// `goway host add`.
pub fn add(
    paths: &Paths,
    renderer: Renderer,
    args: &HostAddArgs,
    lookup: &dyn Lookup,
) -> Result<u8> {
    let config_file = paths.config_file();
    let config = Config::load(&config_file)?;
    if config.host(&args.name).is_ok() {
        return Err(Error::HostAdd {
            name: args.name.clone(),
            reason: "it is already configured (goway host remove it first)".to_owned(),
        });
    }
    let ports: Vec<u16> = args
        .port
        .map_or_else(|| vec![config.defaults.port, 22], |p| vec![p]);
    let trust_address = args
        .address
        .as_deref()
        .is_some_and(|a| a.parse::<std::net::IpAddr>().is_ok());
    let scratch: PathBuf = paths
        .state_dir
        .join(format!("known_hosts.pending.{}", std::process::id()));
    let mut problems = Vec::new();
    for port in ports {
        let _ = std::fs::remove_file(&scratch);
        let candidate = HostConfig {
            name: args.name.clone(),
            address: args.address.clone(),
            port: Some(port),
            user: args.user.clone(),
            max_jobs: args.max_jobs,
        };
        let prober = AddProber {
            inner: SshProber {
                settings: ssh::Settings {
                    known_hosts: scratch.clone(),
                    control_dir: None,
                    connect_timeout_secs: 5,
                },
            },
            name: &args.name,
            address: args.address.as_deref(),
            trust_address,
        };
        renderer.note(format_args!("looking for {} on port {port}", args.name));
        let mut scratch_state = State::default();
        match resolve::resolve(
            &config,
            &candidate,
            &mut scratch_state,
            lookup,
            &prober,
            KeyPolicy::AcceptNew,
            IDENTIFY,
        ) {
            Ok(found) => {
                let id = parse_identity(&found.output).unwrap_or(Identity {
                    os: "Linux".to_owned(),
                    hostname: String::new(),
                    arch: String::new(),
                });
                for finding in sshenv::check(&found.target.address, port) {
                    renderer.warn(finding);
                }
                let alias = config::key_alias(&args.name);
                renderer.note(format_args!("host key: {}", fingerprint(&scratch)));
                adopt_key(&scratch, &paths.known_hosts(), &alias)?;
                let _ = std::fs::remove_file(&scratch);
                let mut stored = candidate.clone();
                if port == config.defaults.port {
                    stored.port = None;
                }
                config::add_host(&config_file, &stored)?;
                let mut state = State::load(&paths.state_file())?;
                state.remember(
                    &args.name,
                    &found.target.address,
                    port,
                    crate::state::now_secs(),
                );
                state.save(&paths.state_file())?;
                renderer.ok(format_args!(
                    "added {} ({} {}, hostname {}) at {}:{port} via {}; key pinned as {alias}",
                    args.name, id.os, id.arch, id.hostname, found.target.address, found.source
                ));
                return Ok(0);
            }
            Err(e) => {
                tracing::info!(port, error = %e, "no usable sshd on this port");
                problems.push(format!("port {port}: {e}"));
            }
        }
    }
    let _ = std::fs::remove_file(&scratch);
    Err(Error::HostAdd {
        name: args.name.clone(),
        reason: format!(
            "no Linux sshd answered as this host\n{}",
            problems.join("\n")
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_identity() {
        assert_eq!(
            parse_identity("Linux\nHelios\nx86_64\n"),
            Some(Identity {
                os: "Linux".to_owned(),
                hostname: "Helios".to_owned(),
                arch: "x86_64".to_owned()
            })
        );
        assert_eq!(parse_identity("Linux\n"), None);
    }

    #[test]
    fn hostname_must_match_name_or_address() {
        assert!(hostname_matches("Helios", "helios", None));
        assert!(hostname_matches(
            "Orion-Notebook",
            "xl",
            Some("Orion-Notebook.local")
        ));
        assert!(!hostname_matches("Orion-Notebook", "helios", None));
        assert!(!hostname_matches("other", "helios", Some("100.1.2.3")));
    }

    // frob:tests crates/goway/src/hosts.rs::adopt_key
    #[test]
    fn adopt_key_replaces_only_the_alias() {
        let dir = tempfile::tempdir().unwrap();
        let scratch = dir.path().join("scratch");
        let kh = dir.path().join("known_hosts");
        let key = "AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl";
        std::fs::write(
            &kh,
            format!("goway-other ssh-ed25519 {key}\ngoway-q ssh-ed25519 {key}\n"),
        )
        .unwrap();
        std::fs::write(&scratch, format!("goway-q ssh-ed25519 {key}\n")).unwrap();
        adopt_key(&scratch, &kh, "goway-q").unwrap();
        let text = std::fs::read_to_string(&kh).unwrap();
        assert_eq!(text.matches("goway-q ").count(), 1, "{text}");
        assert!(text.contains("goway-other"));
        assert!(fingerprint(&kh).contains("SHA256:"));
    }
}
