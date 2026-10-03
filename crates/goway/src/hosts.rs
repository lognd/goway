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
pub fn forget_key(known_hosts: &Path, alias: &str) {
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

/// The SHA256 fingerprints of the keys in a `known_hosts` file.
pub fn fingerprints(known_hosts: &Path) -> Vec<String> {
    let out = Command::new("ssh-keygen")
        .arg("-l")
        .arg("-f")
        .arg(known_hosts)
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default();
    out.split_whitespace()
        .filter(|w| w.starts_with("SHA256:"))
        .map(str::to_owned)
        .collect()
}

/// `SHA256:...` with the prefix added if the user left it out.
pub fn normalize_fingerprint(given: &str) -> String {
    let given = given.trim();
    if given.starts_with("SHA256:") {
        given.to_owned()
    } else {
        format!("SHA256:{given}")
    }
}

/// The command that shows a WSL host's key fingerprint, for the user.
pub const FINGERPRINT_HINT: &str = "ssh-keygen -lf /etc/ssh/ssh_host_ed25519_key.pub";

/// Trust on first use, made explicit: the key the host presented (in
/// `scratch`) is accepted only if it equals `--fingerprint`, or the user
/// confirms it at a terminal. Nothing is pinned and no password is sent
/// otherwise.
pub fn confirm_key(
    renderer: Renderer,
    name: &str,
    address: &str,
    scratch: &Path,
    expected: Option<&str>,
) -> Result<String> {
    let seen = fingerprints(scratch);
    let Some(first) = seen.first().cloned() else {
        return Err(Error::HostAdd {
            name: name.to_owned(),
            reason: format!("{address} presented no host key"),
        });
    };
    if let Some(expected) = expected {
        let expected = normalize_fingerprint(expected);
        return if seen.contains(&expected) {
            tracing::info!(host = name, fingerprint = %expected, "host key matches --fingerprint");
            keep_only(scratch, &expected)?;
            Ok(expected)
        } else {
            Err(Error::HostAdd {
                name: name.to_owned(),
                reason: format!(
                    "{address} presented {first}, not {expected}; it is not the machine you named (or the fingerprint was mistyped)"
                ),
            })
        };
    }
    renderer.headline(format_args!(
        "{address} says it is {name} and presents the host key {first}"
    ));
    renderer.note(format_args!(
        "check it on {name} (in its WSL terminal): {FINGERPRINT_HINT}"
    ));
    match crate::render::ask("Is that the same fingerprint? Pin this key [y/N]: ") {
        Some(answer) if matches!(answer.trim(), "y" | "Y" | "yes" | "YES" | "Yes") => {
            tracing::info!(host = name, fingerprint = %first, "host key confirmed by the user");
            keep_only(scratch, &first)?;
            Ok(first)
        }
        Some(_) => Err(Error::HostAdd {
            name: name.to_owned(),
            reason: "the host key was not confirmed; nothing was pinned".to_owned(),
        }),
        None => Err(Error::HostAdd {
            name: name.to_owned(),
            reason: format!(
                "the host key {first} needs confirmation; run this in a terminal, or pass --fingerprint SHA256:... (get it on {name} with: {FINGERPRINT_HINT})"
            ),
        }),
    }
}

/// Cut the scratch `known_hosts` down to the lines whose key has
/// `fingerprint`, so that pinning adopts exactly what was confirmed and not
/// every key the host happened to present.
fn keep_only(scratch: &Path, fingerprint: &str) -> Result<()> {
    let text = std::fs::read_to_string(scratch).map_err(|e| Error::io("read", scratch, e))?;
    let one = scratch.with_extension("one");
    let mut kept = String::new();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        std::fs::write(&one, format!("{line}\n")).map_err(|e| Error::io("write", &one, e))?;
        if fingerprints(&one).iter().any(|f| f == fingerprint) {
            kept.push_str(line);
            kept.push('\n');
        }
    }
    let _ = std::fs::remove_file(&one);
    if kept.is_empty() {
        return Err(Error::Usage(format!(
            "the confirmed host key {fingerprint} is not in {}",
            scratch.display()
        )));
    }
    tracing::debug!(
        fingerprint,
        "scratch known_hosts narrowed to the confirmed key"
    );
    config::write_atomic(scratch, kept.as_bytes())
}

/// Pin the confirmed key from `scratch`, add the host to the config and
/// remember its address.
pub fn register(
    paths: &Paths,
    host: &HostConfig,
    address: &str,
    port: u16,
    scratch: &Path,
) -> Result<()> {
    let alias = config::key_alias(&host.name);
    adopt_key(scratch, &paths.known_hosts(), &alias)?;
    let _ = std::fs::remove_file(scratch);
    let config = Config::load(&paths.config_file())?;
    let mut stored = host.clone();
    stored.port = (port != config.defaults.port).then_some(port);
    config::add_host(&paths.config_file(), &stored)?;
    let mut state = State::load(&paths.state_file())?;
    state.remember(&host.name, address, port, crate::state::now_secs());
    state.save(&paths.state_file())
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
            priority: None,
            max_load: None,
            identity: None,
            labels: Vec::new(),
            gpu_jobs: None,
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
                if let Err(e) = confirm_key(
                    renderer,
                    &args.name,
                    &found.target.address,
                    &scratch,
                    args.fingerprint.as_deref(),
                ) {
                    let _ = std::fs::remove_file(&scratch);
                    return Err(e);
                }
                register(paths, &candidate, &found.target.address, port, &scratch)?;
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

    #[test]
    fn only_the_confirmed_key_is_kept_for_pinning() {
        let dir = tempfile::tempdir().unwrap();
        let mut lines = Vec::new();
        for n in 0..2 {
            let key = dir.path().join(format!("k{n}"));
            let ok = Command::new("ssh-keygen")
                .args(["-q", "-t", "ed25519", "-N", "", "-f"])
                .arg(&key)
                .status()
                .unwrap();
            assert!(ok.success());
            let public = std::fs::read_to_string(dir.path().join(format!("k{n}.pub"))).unwrap();
            let mut parts = public.split_whitespace();
            lines.push(format!(
                "goway-q {} {}",
                parts.next().unwrap(),
                parts.next().unwrap()
            ));
        }
        let scratch = dir.path().join("scratch");
        std::fs::write(&scratch, lines.join("\n") + "\n").unwrap();
        let fps = fingerprints(&scratch);
        assert_eq!(fps.len(), 2);
        let renderer = Renderer::new(crate::render::ColorWhen::Never);
        let got = confirm_key(renderer, "q", "192.0.2.1", &scratch, Some(&fps[1])).unwrap();
        assert_eq!(got, fps[1]);
        assert_eq!(fingerprints(&scratch), vec![fps[1].clone()]);
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
        assert!(fingerprints(&kh)[0].starts_with("SHA256:"));
        assert_eq!(normalize_fingerprint("abc"), "SHA256:abc");
    }
}
