//! Finding a host without a static IP.
//!
//! A host is a name plus a pinned ssh key; its address is whatever answers
//! with that key right now. Candidates are tried in stages, cheapest first,
//! and each stage is computed only when the previous ones failed (the
//! Windows interop lookup takes seconds):
//!
//! 1. the cached last good address,
//! 2. the configured `address`,
//! 3. the name through the system resolver,
//! 4. `<name>.local` through the system resolver (mDNS where supported),
//! 5. `<name>.local` through Windows, when running under WSL in NAT mode.
//!
//! ssh's own strict host key check rejects every address that is not the
//! pinned host, so a stale or wrong candidate costs one failed probe.

use std::collections::BTreeSet;
use std::net::{IpAddr, ToSocketAddrs};
use std::process::Stdio;
use std::time::{Duration, Instant};

use crate::config::{Config, HostConfig};
use crate::error::{Error, Result};
use crate::ssh::{self, Failure, KeyPolicy, Target};
use crate::state::State;

/// Where a candidate address came from (shown in logs and errors).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Source {
    /// The last address that worked.
    Cached,
    /// The `address` field of the host config.
    Configured,
    /// The host name through the system resolver.
    Name,
    /// `<name>.local` through the system resolver.
    Mdns,
    /// `<name>.local` through Windows (WSL interop).
    WindowsMdns,
    /// This machine, chosen with `--host local` or by the `[local]` pool.
    Local,
    /// This machine, because no helper was reachable and `[local] fallback` is set.
    Fallback,
}

impl std::fmt::Display for Source {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Cached => "cached",
            Self::Configured => "configured",
            Self::Name => "dns",
            Self::Mdns => "mdns",
            Self::WindowsMdns => "windows-mdns",
            Self::Local => "this machine",
            Self::Fallback => "this machine (fallback)",
        })
    }
}

/// Name lookups, injectable so resolution is testable without a network.
pub trait Lookup {
    /// Addresses of `name` through the system resolver.
    fn system(&self, name: &str) -> Vec<IpAddr>;
    /// Addresses of `name` through Windows; empty when not under WSL.
    fn windows(&self, name: &str) -> Vec<IpAddr>;
}

/// The result of one probe of a candidate.
pub type ProbeResult = std::result::Result<String, (Failure, String)>;

/// Runs a remote command against a candidate target, injectable for tests.
pub trait Prober {
    /// Run `remote` on `target` under `policy`; stdout on success, the
    /// classified failure and stderr otherwise.
    fn probe(&self, target: &Target, policy: KeyPolicy, remote: &str) -> ProbeResult;
}

/// A host found at a working address, with the probe's stdout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    /// The working target.
    pub target: Target,
    /// Which stage found it.
    pub source: Source,
    /// stdout of the probe command.
    pub output: String,
}

impl Found {
    /// Whether this is this machine rather than a remote host.
    pub fn is_local(&self) -> bool {
        matches!(self.source, Source::Local | Source::Fallback)
    }
}

/// One failed candidate, for the final error message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Miss {
    /// The address tried.
    pub address: String,
    /// Which stage produced it.
    pub source: Source,
    /// Why it failed.
    pub failure: Failure,
    /// The first line of ssh's or the check's explanation, if any.
    pub detail: String,
}

impl std::fmt::Display for Miss {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let why = match self.failure {
            Failure::HostKeyMismatch => "a different machine (host key mismatch)",
            Failure::HostKeyUnknown => "host key not pinned",
            Failure::AuthRefused => "key authentication refused",
            Failure::Unreachable => "unreachable",
            Failure::Other => "ssh failed",
        };
        if self.detail.is_empty() || self.failure != Failure::Other {
            write!(f, "{} ({}): {why}", self.address, self.source)
        } else {
            write!(f, "{} ({}): {}", self.address, self.source, self.detail)
        }
    }
}

fn addresses(ips: Vec<IpAddr>) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    // IPv4 first: these LANs route IPv4, and ssh tries what it is given.
    let (v4, v6): (Vec<_>, Vec<_>) = ips.into_iter().partition(IpAddr::is_ipv4);
    for ip in v4.into_iter().chain(v6) {
        if ip.is_loopback() || ip.is_unspecified() || is_link_local(ip) {
            continue;
        }
        if seen.insert(ip) {
            out.push(ip.to_string());
        }
    }
    out
}

fn is_link_local(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_link_local(),
        IpAddr::V6(v6) => v6.segments()[0] & 0xffc0 == 0xfe80,
    }
}

/// The addresses of one stage, computed on demand.
fn stage(source: Source, host: &HostConfig, state: &State, lookup: &dyn Lookup) -> Vec<String> {
    let local = format!("{}.local", host.name);
    match source {
        Source::Cached => state
            .get(&host.name)
            .map(|s| vec![s.address.clone()])
            .unwrap_or_default(),
        Source::Configured => match &host.address {
            Some(a) if a.parse::<IpAddr>().is_ok() => vec![a.clone()],
            Some(a) => {
                let mut out = addresses(lookup.system(a));
                if out.is_empty() {
                    out = addresses(lookup.windows(a));
                }
                if out.is_empty() {
                    // Let ssh try the name itself (it may know it via ssh config).
                    out.push(a.clone());
                }
                out
            }
            None => Vec::new(),
        },
        Source::Name => addresses(lookup.system(&host.name)),
        Source::Mdns => addresses(lookup.system(&local)),
        Source::WindowsMdns => addresses(lookup.windows(&local)),
        Source::Local | Source::Fallback => Vec::new(),
    }
}

const STAGES: [Source; 5] = [
    Source::Cached,
    Source::Configured,
    Source::Name,
    Source::Mdns,
    Source::WindowsMdns,
];

/// Find where `host` answers with its pinned key, run `remote` there, and
/// cache the working address in `state` (the caller saves it).
pub fn resolve(
    config: &Config,
    host: &HostConfig,
    state: &mut State,
    lookup: &dyn Lookup,
    prober: &dyn Prober,
    policy: KeyPolicy,
    remote: &str,
) -> Result<Found> {
    let port = config.port_of(host);
    let mut tried = BTreeSet::new();
    let mut misses = Vec::new();
    for source in STAGES {
        let candidates = stage(source, host, state, lookup);
        tracing::debug!(host = %host.name, %source, ?candidates, "resolution stage");
        for address in candidates {
            if !tried.insert(address.clone()) {
                continue;
            }
            let target = Target {
                name: host.name.clone(),
                address: address.clone(),
                port,
                user: host.user.clone(),
                identity: host.identity.as_ref().map(std::path::PathBuf::from),
            };
            match prober.probe(&target, policy, remote) {
                Ok(output) => {
                    tracing::info!(host = %host.name, %address, %source, "host found");
                    state.remember(&host.name, &address, port, crate::state::now_secs());
                    return Ok(Found {
                        target,
                        source,
                        output,
                    });
                }
                Err((failure, stderr)) => {
                    tracing::info!(host = %host.name, %address, %source, ?failure, stderr = stderr.trim(), "candidate failed");
                    let miss = Miss {
                        address,
                        source,
                        failure,
                        detail: stderr
                            .lines()
                            .map(str::trim)
                            .find(|l| !l.is_empty())
                            .unwrap_or_default()
                            .to_owned(),
                    };
                    // An unpinned key or refused auth will not get better at
                    // another address of the same machine; stop early.
                    if matches!(failure, Failure::HostKeyUnknown | Failure::AuthRefused) {
                        misses.push(miss);
                        return Err(Error::HostNotFound {
                            name: host.name.clone(),
                            misses,
                        });
                    }
                    misses.push(miss);
                }
            }
        }
    }
    Err(Error::HostNotFound {
        name: host.name.clone(),
        misses,
    })
}

/// Real lookups: the OS resolver, plus Windows through WSL interop.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemLookup;

impl Lookup for SystemLookup {
    fn system(&self, name: &str) -> Vec<IpAddr> {
        let started = Instant::now();
        let ips = (name, 0)
            .to_socket_addrs()
            .map(|it| it.map(|a| a.ip()).collect())
            .unwrap_or_default();
        tracing::debug!(name, ?ips, elapsed = ?started.elapsed(), "system lookup");
        ips
    }

    fn windows(&self, name: &str) -> Vec<IpAddr> {
        // GOWAY_WINDOWS_LOOKUP=0 turns the interop lookup off (tests, or
        // WSL setups where powershell.exe is not wanted).
        if !under_wsl() || std::env::var_os("GOWAY_WINDOWS_LOOKUP").is_some_and(|v| v == "0") {
            return Vec::new();
        }
        windows_lookup(name)
    }
}

/// True when running inside WSL (where Windows interop may be available).
pub fn under_wsl() -> bool {
    std::env::var_os("WSL_DISTRO_NAME").is_some()
        || std::path::Path::new("/proc/sys/fs/binfmt_misc/WSLInterop").exists()
}

/// Ask Windows to resolve `name` (mDNS, LLMNR, DNS), bounded by a timeout.
fn windows_lookup(name: &str) -> Vec<IpAddr> {
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.' || c == '_')
    {
        return Vec::new();
    }
    let script = format!(
        "Resolve-DnsName -Name '{name}' -Type A -ErrorAction SilentlyContinue | \
         ForEach-Object {{ $_.IPAddress }}"
    );
    let started = Instant::now();
    let child = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn();
    let mut child = match child {
        Ok(c) => c,
        Err(e) => {
            tracing::info!(error = %e, "windows interop unavailable");
            return Vec::new();
        }
    };
    let deadline = started + Duration::from_secs(15);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
            Ok(None) | Err(_) => {
                let _ = child.kill();
                tracing::warn!(name, "windows lookup timed out");
                return Vec::new();
            }
        }
    }
    let mut text = String::new();
    if let Some(mut out) = child.stdout.take() {
        let _ = std::io::Read::read_to_string(&mut out, &mut text);
    }
    let ips = parse_ips(&text);
    tracing::debug!(name, ?ips, elapsed = ?started.elapsed(), "windows lookup");
    ips
}

/// Parse one IP per line, ignoring anything else (CRLF tolerant).
pub fn parse_ips(text: &str) -> Vec<IpAddr> {
    text.lines().filter_map(|l| l.trim().parse().ok()).collect()
}

/// Real probes through the system ssh.
#[derive(Debug, Clone)]
pub struct SshProber {
    /// Shared ssh settings.
    pub settings: ssh::Settings,
}

impl Prober for SshProber {
    fn probe(&self, target: &Target, policy: KeyPolicy, remote: &str) -> ProbeResult {
        let mut cmd = ssh::command(target, &self.settings, policy, remote);
        cmd.stdin(Stdio::null());
        match cmd.output() {
            Ok(out) if out.status.success() => {
                Ok(String::from_utf8_lossy(&out.stdout).into_owned())
            }
            Ok(out) => {
                let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
                let failure = if out.status.code() == Some(255) {
                    ssh::classify_failure(&stderr)
                } else {
                    Failure::Other
                };
                Err((failure, stderr))
            }
            Err(e) => Err((Failure::Other, format!("cannot run ssh: {e}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::BTreeMap;

    #[derive(Default)]
    struct FakeLookup {
        system: BTreeMap<String, Vec<IpAddr>>,
        windows: BTreeMap<String, Vec<IpAddr>>,
        windows_calls: RefCell<u32>,
    }

    impl Lookup for FakeLookup {
        fn system(&self, name: &str) -> Vec<IpAddr> {
            self.system.get(name).cloned().unwrap_or_default()
        }
        fn windows(&self, name: &str) -> Vec<IpAddr> {
            *self.windows_calls.borrow_mut() += 1;
            self.windows.get(name).cloned().unwrap_or_default()
        }
    }

    /// Answers only at `good` and records every address probed.
    struct FakeProber {
        good: String,
        wrong_machine: Vec<String>,
        probed: RefCell<Vec<String>>,
    }

    impl Prober for FakeProber {
        fn probe(&self, target: &Target, _: KeyPolicy, _: &str) -> ProbeResult {
            self.probed.borrow_mut().push(target.address.clone());
            if target.address == self.good {
                Ok("ok\n".to_owned())
            } else if self.wrong_machine.contains(&target.address) {
                Err((Failure::HostKeyMismatch, String::new()))
            } else {
                Err((Failure::Unreachable, String::new()))
            }
        }
    }

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    fn host() -> HostConfig {
        HostConfig {
            name: "helios".to_owned(),
            ..HostConfig::default()
        }
    }

    #[test]
    fn stale_cache_falls_through_to_windows_mdns_and_is_recached() {
        let config = Config::default();
        let mut state = State::default();
        state.remember("helios", "192.0.2.1", 2222, 1);
        let mut lookup = FakeLookup::default();
        lookup.windows.insert(
            "helios.local".to_owned(),
            vec![ip("192.168.56.1"), ip("192.0.2.10"), ip("169.254.1.1")],
        );
        let prober = FakeProber {
            good: "192.0.2.10".to_owned(),
            wrong_machine: vec!["192.168.56.1".to_owned()],
            probed: RefCell::new(Vec::new()),
        };
        let found = resolve(
            &config,
            &host(),
            &mut state,
            &lookup,
            &prober,
            KeyPolicy::Strict,
            "true",
        )
        .unwrap();
        assert_eq!(found.target.address, "192.0.2.10");
        assert_eq!(found.source, Source::WindowsMdns);
        assert_eq!(
            *prober.probed.borrow(),
            ["192.0.2.1", "192.168.56.1", "192.0.2.10"],
            "link-local skipped, order kept"
        );
        assert_eq!(state.get("helios").unwrap().address, "192.0.2.10");
    }

    #[test]
    fn working_cache_never_touches_slow_lookups() {
        let config = Config::default();
        let mut state = State::default();
        state.remember("helios", "192.0.2.10", 2222, 1);
        let lookup = FakeLookup::default();
        let prober = FakeProber {
            good: "192.0.2.10".to_owned(),
            wrong_machine: Vec::new(),
            probed: RefCell::new(Vec::new()),
        };
        let found = resolve(
            &config,
            &host(),
            &mut state,
            &lookup,
            &prober,
            KeyPolicy::Strict,
            "true",
        )
        .unwrap();
        assert_eq!(found.source, Source::Cached);
        assert_eq!(*lookup.windows_calls.borrow(), 0);
    }

    #[test]
    fn configured_then_dns_then_mdns_order_and_dedup() {
        let config = Config::default();
        let mut state = State::default();
        let mut h = host();
        h.address = Some("helios-box".to_owned());
        let mut lookup = FakeLookup::default();
        lookup
            .system
            .insert("helios-box".to_owned(), vec![ip("10.0.0.1")]);
        lookup
            .system
            .insert("helios".to_owned(), vec![ip("10.0.0.1"), ip("10.0.0.2")]);
        lookup
            .system
            .insert("helios.local".to_owned(), vec![ip("10.0.0.3")]);
        let prober = FakeProber {
            good: "10.0.0.3".to_owned(),
            wrong_machine: Vec::new(),
            probed: RefCell::new(Vec::new()),
        };
        let found = resolve(
            &config,
            &h,
            &mut state,
            &lookup,
            &prober,
            KeyPolicy::Strict,
            "true",
        )
        .unwrap();
        assert_eq!(found.source, Source::Mdns);
        assert_eq!(
            *prober.probed.borrow(),
            ["10.0.0.1", "10.0.0.2", "10.0.0.3"]
        );
    }

    #[test]
    fn nothing_found_lists_every_miss() {
        let config = Config::default();
        let mut state = State::default();
        state.remember("helios", "10.9.9.9", 2222, 1);
        let lookup = FakeLookup::default();
        let prober = FakeProber {
            good: "none".to_owned(),
            wrong_machine: Vec::new(),
            probed: RefCell::new(Vec::new()),
        };
        let e = resolve(
            &config,
            &host(),
            &mut state,
            &lookup,
            &prober,
            KeyPolicy::Strict,
            "true",
        )
        .unwrap_err();
        let text = e.to_string();
        assert!(text.contains("10.9.9.9 (cached): unreachable"), "{text}");
    }

    #[test]
    fn parses_windows_output_with_crlf() {
        assert_eq!(
            parse_ips("192.0.2.10\r\n\r\nfe80::1\r\nnoise\r\n"),
            vec![ip("192.0.2.10"), ip("fe80::1")]
        );
    }

    // frob:tests crates/goway/src/resolve.rs::SystemLookup.system
    #[test]
    fn system_lookup_resolves_localhost() {
        assert!(
            SystemLookup
                .system("localhost")
                .iter()
                .any(IpAddr::is_loopback)
        );
        assert!(
            addresses(SystemLookup.system("localhost")).is_empty(),
            "loopback is never a host"
        );
    }

    // frob:tests crates/goway/src/resolve.rs::SystemLookup.windows
    // frob:tests crates/goway/src/resolve.rs::under_wsl
    #[test]
    fn windows_lookup_rejects_unsafe_names_and_is_empty_off_wsl() {
        assert!(windows_lookup("x'; rm -rf /").is_empty());
        if !under_wsl() {
            assert!(SystemLookup.windows("anything.local").is_empty());
        }
    }

    #[test]
    fn windows_interop_answers_under_wsl() {
        // WSL interop is absent in ssh sessions (and on goway hosts); only
        // check the answer where powershell.exe can actually run.
        let interop = std::process::Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", "exit 0"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success());
        if !under_wsl() || !interop {
            return;
        }
        let ips = windows_lookup("localhost");
        assert!(ips.iter().any(IpAddr::is_loopback), "{ips:?}");
    }
}
