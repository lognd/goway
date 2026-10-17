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
use crate::spawn::CommandExt as _;
use crate::ssh::{self, Failure, KeyPolicy, Target};
use crate::sshenv;
use crate::state::State;

/// The probe `host add` runs: OS, hostname, arch, one per line.
pub const IDENTIFY: &str = "uname -s; uname -n; uname -m";

/// The probe for a Windows OpenSSH server (whose shell is cmd.exe or
/// PowerShell and has no `uname`): the same three lines, `Windows` first.
pub const IDENTIFY_WINDOWS: &str = "powershell -NoProfile -NonInteractive -Command \"'Windows'; $env:COMPUTERNAME; $env:PROCESSOR_ARCHITECTURE\"";

// frob:ticket 01M544DC8P7E0DTBA25QMYNF4Q
/// The probe output recorded when a Windows sshd was recognised by its
/// banner but refused goway's key, so it could not be asked who it is.
pub const WINDOWS_KEY_REFUSED: &str = "Windows\n?\n?\n";

// frob:ticket 01M544DC8P7E0DTBA25QMYNF4Q
/// Whether an ssh banner line (`SSH-2.0-OpenSSH_for_Windows_9.5`) is Windows' OpenSSH.
pub fn is_windows_banner(banner: &str) -> bool {
    banner.starts_with("SSH-") && banner.to_ascii_lowercase().contains("windows")
}

// frob:ticket 01M544DC8P7E0DTBA25QMYNF4Q
/// Read the first line a sshd sends (its banner); `None` if nothing answers in time.
pub fn read_banner(address: &str, port: u16) -> Option<String> {
    use std::io::{BufRead as _, BufReader};
    use std::net::{TcpStream, ToSocketAddrs as _};
    use std::time::Duration;
    let addr = (address, port).to_socket_addrs().ok()?.next()?;
    let stream = TcpStream::connect_timeout(&addr, Duration::from_secs(3)).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(3))).ok()?;
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line).ok()?;
    let line = line.trim().to_owned();
    tracing::debug!(address, port, banner = %line, "ssh banner read");
    Some(line)
}

// frob:ticket 01M544DC8P7E0DTBA25QMYNF4Q
/// The commands that make a Windows machine a goway helper over OpenSSH,
/// run in PowerShell as Administrator; `key` is goway's public key line.
/// (Printed text lives in `winsetup.txt`: goway runs none of it itself.)
pub fn windows_setup_commands(key: &str) -> String {
    include_str!("winsetup.txt")
        .trim_end()
        .replace("{KEY}", key)
}

// frob:ticket 01M544DC8P7E0DTBA25QMYNF4Q
/// What to do about a Windows OpenSSH server that refused goway's key.
pub fn windows_refused_advice(name: &str, key: &str) -> String {
    format!(
        "{name} runs the Windows OpenSSH server and does not accept goway's key yet. (Linux password and lockout advice does not apply to it.) On {name}, in PowerShell as Administrator:\n{}\n    then check with `goway doctor {name}` (or run `goway-setup install --host --native --authorized-key \"{key}\"` there)",
        windows_setup_commands(key)
    )
}

/// goway's public key line from its config dir, or a placeholder to fill in.
fn public_key_line(paths: &Paths) -> String {
    std::fs::read_to_string(paths.config_dir.join("id_ed25519.pub"))
        .map(|t| t.trim().to_owned())
        .ok()
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| "ssh-ed25519 AAAA... goway@laptop".to_owned())
}

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
        arch: normalize_arch(lines.next()?),
    })
}

/// `uname -m` spelling for the names Windows uses (`AMD64`, `ARM64`).
fn normalize_arch(arch: &str) -> String {
    match arch.to_ascii_uppercase().as_str() {
        "AMD64" => "x86_64".to_owned(),
        "ARM64" => "aarch64".to_owned(),
        _ => arch.to_owned(),
    }
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
    /// Accept a Windows OpenSSH server (second pass: no Linux machine answered).
    accept_windows: bool,
    /// A Windows server answered as the intended machine and was passed over.
    windows_seen: std::cell::Cell<bool>,
    /// Reads a sshd's banner (faked in tests).
    banner: &'a dyn Fn(&str, u16) -> Option<String>,
    /// The user vouches for the machine (an IP, or a fingerprint to check),
    /// which a machine that refuses the key cannot confirm by hostname.
    vouched: bool,
}

impl AddProber<'_> {
    /// A sshd refused the key: if its banner says Windows, keep its host key
    /// (pass 1 only, and only when vouched for) so it can be recorded.
    fn refused(&self, target: &Target, stderr: &str) -> Option<ProbeResult> {
        let banner = (self.banner)(&target.address, target.port)?;
        if !is_windows_banner(&banner) {
            return None;
        }
        tracing::info!(address = %target.address, port = target.port, %banner, "Windows sshd refused the key");
        let drop_key = || {
            forget_scratch_key(
                &self.inner.settings.known_hosts,
                &config::key_alias(self.name),
            );
        };
        if !self.vouched {
            drop_key();
            return Some(Err((
                Failure::AuthRefused,
                format!(
                    "{} is a Windows OpenSSH server that refused the key, so goway cannot check its hostname: pass --address with its IP or --fingerprint to vouch for it ({stderr})",
                    target.address
                ),
            )));
        }
        if !self.accept_windows {
            self.windows_seen.set(true);
            drop_key();
            return Some(Err((
                Failure::AuthRefused,
                format!(
                    "{} is Windows, not Linux; kept as a fallback",
                    target.address
                ),
            )));
        }
        Some(Ok(WINDOWS_KEY_REFUSED.to_owned()))
    }

    /// Identify a Windows sshd with PowerShell; accept it only on the
    /// second pass and only if it is the intended machine, so the Windows
    /// side of a machine is never taken for its WSL host (or the reverse).
    fn windows(&self, target: &Target, policy: KeyPolicy, stderr: &str) -> ProbeResult {
        let reject = |why: String| {
            forget_scratch_key(
                &self.inner.settings.known_hosts,
                &config::key_alias(self.name),
            );
            Err((Failure::Other, why))
        };
        let Ok(text) = self.inner.probe(target, policy, IDENTIFY_WINDOWS) else {
            return reject(format!(
                "{} is Windows and gave no identity: {stderr}",
                target.address
            ));
        };
        match parse_identity(&text) {
            Some(id) if id.os == "Windows" => {
                if !self.trust_address && !hostname_matches(&id.hostname, self.name, self.address) {
                    return reject(format!(
                        "{} is `{}`, not `{}` (pass --address with its IP to trust it anyway)",
                        target.address, id.hostname, self.name
                    ));
                }
                if !self.accept_windows {
                    self.windows_seen.set(true);
                    return reject(format!(
                        "{} is Windows, not Linux; kept as a fallback",
                        target.address
                    ));
                }
                Ok(text)
            }
            _ => reject(format!("{} gave no usable identity", target.address)),
        }
    }
}

impl Prober for AddProber<'_> {
    fn probe(&self, target: &Target, policy: KeyPolicy, remote: &str) -> ProbeResult {
        let out = self.inner.probe(target, policy, remote);
        let reject = |why: String| {
            forget_scratch_key(
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
            Err((Failure::AuthRefused, stderr)) => {
                if let Some(result) = self.refused(target, &stderr) {
                    return result;
                }
                forget_scratch_key(
                    &self.inner.settings.known_hosts,
                    &config::key_alias(self.name),
                );
                Err((Failure::AuthRefused, stderr))
            }
            Err((failure, stderr)) => {
                forget_scratch_key(
                    &self.inner.settings.known_hosts,
                    &config::key_alias(self.name),
                );
                // A Windows sshd runs `uname` in cmd.exe and fails with 1:
                // ask it again in PowerShell.
                if failure == Failure::Other && stderr.contains("not recognized") {
                    tracing::info!(address = %target.address, port = target.port, "Windows sshd answered");
                    return self.windows(target, policy, &stderr);
                }
                Err((failure, stderr))
            }
        }
    }
}

/// Whether a `known_hosts` line pins `alias` (the first field is a comma-separated name list).
fn names_alias(line: &str, alias: &str) -> bool {
    line.split_whitespace()
        .next()
        .is_some_and(|names| names.split(',').any(|n| n == alias))
}

/// Remove `alias` from goway's pinned `known_hosts` (no-op if absent), recording the edit in the
/// change log so it can be listed and undone. A failure is logged: forgetting is best effort.
pub fn forget_key(known_hosts: &Path, alias: &str) {
    let Ok(text) = std::fs::read_to_string(known_hosts) else {
        return;
    };
    let kept: String = text
        .split_inclusive('\n')
        .filter(|line| !names_alias(line, alias))
        .collect();
    if kept == text {
        return;
    }
    match crate::changelog::write_file(known_hosts, kept.as_bytes()) {
        Ok(()) => tracing::debug!(alias, file = %known_hosts.display(), "key forgotten"),
        Err(e) => tracing::error!(alias, error = %e, "could not forget the pinned key"),
    }
}

/// Remove `alias` from a scratch `known_hosts` of one probe run (goway's own run state, no
/// change to record), with `ssh-keygen -R` (no-op if absent).
fn forget_scratch_key(known_hosts: &Path, alias: &str) {
    if !known_hosts.exists() {
        return;
    }
    let result = Command::new("ssh-keygen")
        .arg("-R")
        .arg(alias)
        .arg("-f")
        .arg(known_hosts)
        .stdin(std::process::Stdio::null())
        .output_locked();
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
        .stdin(std::process::Stdio::null())
        .output_locked()
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
    let unset = HostConfig {
        port: None,
        ..stored.clone()
    };
    stored.port = (port != config.port_of(&unset)).then_some(port);
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
    crate::changelog::write_file(known_hosts, current.as_bytes())?;
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
    add_with(paths, renderer, args, lookup, &read_banner)
}

/// [`add`] with the banner reader injected, so tests need no network.
#[allow(clippy::too_many_lines)] // one sequence: probe each port, confirm the key, register
fn add_with(
    paths: &Paths,
    renderer: Renderer,
    args: &HostAddArgs,
    lookup: &dyn Lookup,
    banner: &dyn Fn(&str, u16) -> Option<String>,
) -> Result<u8> {
    if args.windows {
        // Print-only: nothing is probed, pinned or written.
        renderer.line(windows_refused_advice(&args.name, &public_key_line(paths)));
        tracing::info!(host = %args.name, "printed the Windows setup commands");
        return Ok(0);
    }
    crate::ssh::attempts::init(&paths.state_dir);
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
    let mut windows_ports: Vec<u16> = Vec::new();
    // Pass 0 looks for the Linux machine (its WSL sshd) on every port; only
    // when none answers does pass 1 accept a Windows OpenSSH server that
    // answered as the same machine, so the two are never confused.
    for pass in 0..2u8 {
        let accept_windows = pass == 1;
        let list = if accept_windows {
            std::mem::take(&mut windows_ports)
        } else {
            ports.clone()
        };
        for port in list {
            let _ = std::fs::remove_file(&scratch);
            let candidate = HostConfig {
                name: args.name.clone(),
                address: args.address.clone(),
                port: Some(port),
                user: args.user.clone(),
                max_jobs: args.max_jobs,
                ..HostConfig::default()
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
                accept_windows,
                windows_seen: std::cell::Cell::new(false),
                banner,
                vouched: trust_address || args.fingerprint.is_some(),
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
                    let mut candidate = candidate;
                    if id.os == "Windows" {
                        candidate.os = config::Os::Windows;
                    }
                    let key_refused = found.output == WINDOWS_KEY_REFUSED;
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
                    if key_refused {
                        renderer.warn(windows_refused_advice(&args.name, &public_key_line(paths)));
                    }
                    return Ok(0);
                }
                Err(e) => {
                    tracing::info!(port, error = %e, "no usable sshd on this port");
                    if !accept_windows && prober.windows_seen.get() {
                        windows_ports.push(port);
                    }
                    problems.push(format!("port {port}: {e}"));
                }
            }
        }
    }
    let _ = std::fs::remove_file(&scratch);
    Err(Error::HostAdd {
        name: args.name.clone(),
        reason: format!(
            "no Linux sshd answered as this host (a Windows OpenSSH server is accepted too: `goway host add {} --windows` prints its setup commands)\n{}",
            args.name,
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

    fn fake_prober<'a>(
        dir: &Path,
        banner: &'a dyn Fn(&str, u16) -> Option<String>,
        accept_windows: bool,
        vouched: bool,
    ) -> AddProber<'a> {
        AddProber {
            inner: SshProber {
                settings: ssh::Settings {
                    known_hosts: dir.join("scratch"),
                    control_dir: None,
                    connect_timeout_secs: 1,
                },
            },
            name: "nova",
            address: None,
            trust_address: vouched,
            accept_windows,
            windows_seen: std::cell::Cell::new(false),
            banner,
            vouched,
        }
    }

    fn target() -> Target {
        Target {
            name: "nova".to_owned(),
            address: "192.0.2.9".to_owned(),
            port: 22,
            user: None,
            identity: None,
        }
    }

    // frob:tests crates/goway/src/hosts.rs::is_windows_banner
    // frob:tests crates/goway/src/hosts.rs::read_banner
    #[test]
    fn a_banner_reader_gets_the_first_line_a_server_sends() {
        use std::io::Write as _;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let (mut c, _) = listener.accept().unwrap();
            c.write_all(b"SSH-2.0-OpenSSH_for_Windows_9.5\r\n").unwrap();
        });
        assert_eq!(
            read_banner("127.0.0.1", port).as_deref(),
            Some("SSH-2.0-OpenSSH_for_Windows_9.5")
        );
    }

    // frob:tests crates/goway/src/hosts.rs::is_windows_banner
    #[test]
    fn windows_banner_is_recognised() {
        assert!(is_windows_banner("SSH-2.0-OpenSSH_for_Windows_9.5"));
        assert!(!is_windows_banner("SSH-2.0-OpenSSH_9.6p1 Ubuntu-3"));
        assert!(!is_windows_banner("HTTP/1.1 windows"));
    }

    // frob:tests crates/goway/src/hosts.rs::windows_refused_advice
    #[test]
    fn windows_advice_is_windows_specific() {
        let text = windows_refused_advice("nova", "ssh-ed25519 AAAA k");
        for needle in [
            "OpenSSH.Server",
            "administrators_authorized_keys",
            "icacls.exe",
            "DefaultShell",
            "powershell.exe",
            "ssh-ed25519 AAAA k",
        ] {
            assert!(text.contains(needle), "missing {needle}: {text}");
        }
        assert!(
            !text.contains("passwd -S") && !text.contains("faillock"),
            "{text}"
        );
    }

    // frob:tests crates/goway/src/hosts.rs::AddProber
    #[test]
    fn a_windows_sshd_that_refuses_the_key_is_taken_only_as_the_fallback() {
        let dir = tempfile::tempdir().unwrap();
        let win = |_: &str, _: u16| Some("SSH-2.0-OpenSSH_for_Windows_9.5".to_owned());
        let linux = |_: &str, _: u16| Some("SSH-2.0-OpenSSH_9.6".to_owned());
        let none = |_: &str, _: u16| None;
        // Pass 0: remembered for pass 1, not accepted.
        let p = fake_prober(dir.path(), &win, false, true);
        assert!(p.refused(&target(), "denied").unwrap().is_err());
        assert!(p.windows_seen.get());
        // Pass 1: accepted with the marker.
        let p = fake_prober(dir.path(), &win, true, true);
        assert_eq!(
            p.refused(&target(), "denied").unwrap(),
            Ok(WINDOWS_KEY_REFUSED.to_owned())
        );
        // Not vouched for: refused with the reason.
        let p = fake_prober(dir.path(), &win, true, false);
        let Err((_, why)) = p.refused(&target(), "denied").unwrap() else {
            panic!("must refuse an unvouched machine");
        };
        assert!(why.contains("--fingerprint"), "{why}");
        // A Linux banner or silence is not Windows.
        assert!(
            fake_prober(dir.path(), &linux, true, true)
                .refused(&target(), "d")
                .is_none()
        );
        assert!(
            fake_prober(dir.path(), &none, true, true)
                .refused(&target(), "d")
                .is_none()
        );
        assert_eq!(
            parse_identity(WINDOWS_KEY_REFUSED).map(|i| i.os),
            Some("Windows".to_owned())
        );
    }

    // frob:tests crates/goway/src/hosts.rs::add
    #[test]
    fn host_add_windows_only_prints() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths {
            config_dir: dir.path().join("c"),
            state_dir: dir.path().join("s"),
            runtime_dir: None,
        };
        std::fs::create_dir_all(&paths.config_dir).unwrap();
        std::fs::write(
            paths.config_dir.join("id_ed25519.pub"),
            "ssh-ed25519 KEY goway@x\n",
        )
        .unwrap();
        assert_eq!(public_key_line(&paths), "ssh-ed25519 KEY goway@x");
        let args = HostAddArgs {
            name: "nova".to_owned(),
            address: None,
            port: None,
            user: None,
            max_jobs: None,
            fingerprint: None,
            windows: true,
        };
        let renderer = Renderer::new(crate::render::ColorWhen::Never);
        let banner = |_: &str, _: u16| -> Option<String> { panic!("no network in --windows") };
        assert_eq!(
            add_with(&paths, renderer, &args, &resolve::SystemLookup, &banner).unwrap(),
            0
        );
        assert!(
            !paths.config_file().exists(),
            "print-only must not write the config"
        );
    }

    // frob:tests crates/goway/src/hosts.rs::register
    #[test]
    fn a_windows_ssh_helper_is_recorded_and_listed() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths {
            config_dir: dir.path().join("c"),
            state_dir: dir.path().join("s"),
            runtime_dir: None,
        };
        std::fs::create_dir_all(&paths.config_dir).unwrap();
        std::fs::create_dir_all(&paths.state_dir).unwrap();
        let key = "AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl";
        let scratch = dir.path().join("scratch");
        std::fs::write(&scratch, format!("goway-nova ssh-ed25519 {key}\n")).unwrap();
        let host = HostConfig {
            name: "nova".to_owned(),
            os: config::Os::Windows,
            ..HostConfig::default()
        };
        register(&paths, &host, "192.0.2.9", 22, &scratch).unwrap();
        let config = Config::load(&paths.config_file()).unwrap();
        let got = config.host("nova").unwrap();
        assert_eq!(got.os, config::Os::Windows);
        assert!(fingerprints(&paths.known_hosts())[0].starts_with("SHA256:"));
    }
}
