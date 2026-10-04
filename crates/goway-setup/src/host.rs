//! The host component's plan: turn a Windows machine with WSL2 into a goway build host.
//!
//! Pure construction only (no probing, no side effects), so the plan is unit and property tested
//! against the model system. Windows-side changes use the ordinary `Change` vocabulary with
//! Windows paths; WSL-side changes use the same vocabulary with absolute `/unix/paths`, which
//! [`crate::hostsys::HostSystem`] routes into the distro (see its module docs).

use std::path::PathBuf;

use goway_journal::{Change, Entry, Journal, Prior, RegValue, ResourceKind};
use serde::{Deserialize, Serialize};

use crate::entry::host_uninstall_values;
use crate::layout::{DEFAULT_PROFILE, Layout};
use crate::relay::{
    self, LISTEN_ADDRESS, PortProxyRule, PortProxySpec, REFRESH_MINUTES, RelayTaskSpec,
};

/// Default TCP port of the WSL sshd.
pub const DEFAULT_PORT: u16 = 2222;
/// Default WSL distro.
pub const DEFAULT_DISTRO: &str = "Ubuntu";
/// The WSL VM's Hyper-V firewall creator id (the same on every machine).
pub const WSL_VM_CREATOR_ID: &str = "{40E0AC32-46A5-438A-A0B2-2B479E8F2E90}";
/// The sshd package goway installs when absent.
pub const SSHD_PACKAGE: &str = "openssh-server";
/// Where the distro reads sshd drop-ins.
pub const SSHD_DROPIN_DIR: &str = "/etc/ssh/sshd_config.d";
/// The distro's WSL settings file.
pub const WSL_CONF: &str = "/etc/wsl.conf";
/// The firewall profiles the rules apply to by default: networks the user marked Private, and
/// domain networks. Public networks (cafe Wi-Fi) are deliberately excluded.
pub const FIREWALL_PROFILES: &str = "Private,Domain";
/// The firewall keyword for "the subnet(s) this machine is directly attached to".
pub const LOCAL_SUBNET: &str = "LocalSubnet";
/// The systemd units enabled so sshd starts with the distro.
pub const SSHD_UNITS: [&str; 2] = ["ssh.socket", "ssh.service"];

/// When the keepalive task starts the distro.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum Keepalive {
    /// At the user's logon, as the invoking user (no administrator rights for the task itself).
    #[default]
    Logon,
    /// At boot without a logon (the task runs with `S4U`; needs administrator rights to register).
    Boot,
}

/// Which WSL networking the user asked for (`--network`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, clap::ValueEnum)]
pub enum NetworkChoice {
    /// Mirrored when the Windows build supports it and `.wslconfig` does not say `nat`; else NAT.
    #[default]
    Auto,
    /// Set `networkingMode=mirrored` (Windows 11 22H2 or newer only).
    Mirrored,
    /// Leave WSL in NAT mode and relay the port through Windows (`netsh portproxy`).
    Nat,
}

/// The networking the install actually set up; recorded so uninstall and status know it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NetworkMode {
    /// WSL mirrors the Windows network: the firewall rules alone expose sshd.
    #[default]
    Mirrored,
    /// WSL sits behind NAT: a portproxy relay (kept current by a task) exposes sshd.
    Nat,
}

impl NetworkMode {
    /// The lowercase name used on the command line and in messages.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Mirrored => "mirrored",
            Self::Nat => "nat",
        }
    }
}

/// First Windows build with mirrored networking (Windows 11 22H2).
pub const MIRRORED_MIN_BUILD: u32 = 22621;

/// Decide the networking mode from the request and what the machine is like.
///
/// `auto` picks mirrored only when the build supports it and the user has not explicitly set
/// `networkingMode=nat` in `.wslconfig`; an explicit `mirrored` on an older build is refused.
pub fn resolve_network(
    choice: NetworkChoice,
    facts: &HostFacts,
) -> Result<NetworkMode, crate::error::SetupError> {
    let supported = facts.windows_build.is_some_and(|b| b >= MIRRORED_MIN_BUILD);
    let mode = match choice {
        NetworkChoice::Nat => NetworkMode::Nat,
        NetworkChoice::Mirrored if supported => NetworkMode::Mirrored,
        NetworkChoice::Mirrored => {
            tracing::warn!(build = ?facts.windows_build, "mirrored networking refused: build too old");
            return Err(crate::error::SetupError::MirroredUnsupported {
                build: facts.windows_build,
            });
        }
        NetworkChoice::Auto => {
            let explicit_nat = facts
                .wslconfig_network
                .as_deref()
                .is_some_and(|m| m.eq_ignore_ascii_case("nat"));
            if supported && !explicit_nat {
                NetworkMode::Mirrored
            } else {
                NetworkMode::Nat
            }
        }
    };
    tracing::info!(?choice, ?mode, build = ?facts.windows_build, wslconfig = ?facts.wslconfig_network, "resolved networking mode");
    Ok(mode)
}

/// Refuse a NAT install when a portproxy rule already listens on the helper's port.
///
/// goway never edits or removes a rule it did not create, so the user must pick another
/// `--port` or remove the rule themselves.
pub fn check_relay_port(
    mode: NetworkMode,
    port: u16,
    rules: &[PortProxyRule],
) -> Result<(), crate::error::SetupError> {
    if mode != NetworkMode::Nat {
        return Ok(());
    }
    match relay::rule_on_port(rules, port) {
        Some(r) => {
            tracing::warn!(
                ?r,
                "install refused: a portproxy rule already uses the port"
            );
            Err(crate::error::SetupError::RelayPortBusy {
                port,
                listen: r.listen_address.to_string(),
                connect: format!("{}:{}", r.connect_address, r.connect_port),
            })
        }
        None => Ok(()),
    }
}

/// What to tell the user after a NAT-mode install about how the helper is reached.
pub fn nat_notice(port: u16) -> String {
    format!(
        "network mode: nat. Other computers reach the helper through a Windows relay on port {port} (netsh portproxy), \
         limited by the same firewall rules; a scheduled task re-points it whenever WSL's address changes"
    )
}

/// What the user asked the host component to set up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostParams {
    /// TCP port the WSL sshd listens on.
    pub port: u16,
    /// WSL distro name.
    pub distro: String,
    /// When the keepalive task starts.
    pub keepalive: Keepalive,
    /// Add a drop-in disabling password authentication (only applied when a key is authorized).
    pub harden: bool,
    /// Extra remote addresses (validated CIDRs) allowed in besides the local subnet.
    pub allow_from: Vec<String>,
    /// The user's home directory, where `.wslconfig` lives.
    pub home: PathBuf,
    /// The resolved networking mode (see [`resolve_network`]).
    pub network: NetworkMode,
}

/// What probing the machine found; it decides which optional steps the plan contains.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostFacts {
    /// Whether the Hyper-V firewall cmdlets exist (Windows 11 22H2 or later).
    pub hyperv_firewall: bool,
    /// Ports the distro's sshd already listens on by configuration (empty when not installed).
    pub sshd_ports: Vec<u16>,
    /// Whether the distro's default user has at least one authorized key.
    pub authorized_keys: bool,
    /// The Windows build number, when it could be read.
    pub windows_build: Option<u32>,
    /// `networkingMode` already set in the user's `.wslconfig`, if any.
    pub wslconfig_network: Option<String>,
    /// The machine's current portproxy rules.
    pub portproxy: Vec<PortProxyRule>,
}

impl HostFacts {
    /// What a dry run assumes on a fresh, capable machine.
    pub fn assumed() -> Self {
        Self {
            hyperv_firewall: true,
            sshd_ports: Vec::new(),
            authorized_keys: true,
            windows_build: Some(MIRRORED_MIN_BUILD + 10),
            wslconfig_network: None,
            portproxy: Vec::new(),
        }
    }
}

/// Settings persisted next to the host journal so uninstall can reach the same distro.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostSettings {
    /// WSL distro the install changed.
    pub distro: String,
    /// The sshd port the install configured.
    pub port: u16,
    /// Extra remote addresses the firewall rules admit besides the local subnet.
    #[serde(default)]
    pub allow_from: Vec<String>,
    /// The networking mode the install chose (journals before NAT support are mirrored).
    #[serde(default)]
    pub network: NetworkMode,
    /// Set for a native (no WSL) install: what its plan was built from. The distro and port
    /// fields then hold the defaults and are not used.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native: Option<crate::native::NativeSettings>,
}

impl Default for HostSettings {
    /// What an uninstall assumes when no settings were saved: the default distro and port.
    fn default() -> Self {
        Self {
            distro: DEFAULT_DISTRO.to_owned(),
            port: DEFAULT_PORT,
            allow_from: Vec::new(),
            network: NetworkMode::Mirrored,
            native: None,
        }
    }
}

impl HostSettings {
    /// Refuse settings an elevated process must not build its expected plan from.
    pub fn validate(&self, path: &std::path::Path) -> Result<(), crate::error::SetupError> {
        if let Some(native) = &self.native {
            native.validate(path)?;
        } else {
            validate_distro(&self.distro)?;
        }
        // A wide range may have been chosen on purpose at install time (`--allow-wide`), so
        // replaying must not refuse it; the never-accepted ranges stay refused.
        for cidr in &self.allow_from {
            validate_allow_from_with(cidr, true)?;
        }
        if self.port == 0 {
            return Err(crate::error::SetupError::UntrustedState {
                path: path.display().to_string(),
                reason: "port 0 is not a valid sshd port".to_owned(),
            });
        }
        Ok(())
    }
}

/// Where a firewall rule applies. `None`/empty is how journals written before scoping existed
/// deserialize (and serialize back unchanged); creating a rule treats them as the safe default.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Scope {
    /// Comma-separated firewall profiles (`Private,Domain`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profiles: Option<String>,
    /// Remote addresses the rule admits (`LocalSubnet`, CIDRs).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub remote_addresses: Vec<String>,
}

impl Scope {
    /// The scope of new rules: Private and Domain profiles, the local subnet plus `allow_from`.
    pub fn new(allow_from: &[String]) -> Self {
        let mut remote_addresses = vec![LOCAL_SUBNET.to_owned()];
        for cidr in allow_from {
            if !remote_addresses.contains(cidr) {
                remote_addresses.push(cidr.clone());
            }
        }
        Self {
            profiles: Some(FIREWALL_PROFILES.to_owned()),
            remote_addresses,
        }
    }
}

/// Spec of a Windows Defender Firewall rule resource (JSON in `Change::EnsureResource::spec`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FirewallSpec {
    /// Inbound TCP port to allow.
    pub port: u16,
    /// Free-text description stored on the rule.
    pub description: String,
    /// Profiles and remote addresses the rule is limited to.
    #[serde(flatten)]
    pub scope: Scope,
}

/// Spec of a Hyper-V firewall rule resource.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HyperVSpec {
    /// Inbound TCP port to allow.
    pub port: u16,
    /// The VM creator the rule applies to.
    pub vm_creator_id: String,
    /// Free-text description stored on the rule.
    pub description: String,
    /// Profiles and remote addresses the rule is limited to.
    #[serde(flatten)]
    pub scope: Scope,
}

/// Shortest IPv4 prefix `--allow-from` accepts without `--allow-wide`.
pub const MIN_PREFIX_V4: u8 = 8;
/// Shortest IPv6 prefix `--allow-from` accepts without `--allow-wide`.
pub const MIN_PREFIX_V6: u8 = 16;

/// Accept an `--allow-from` value: an IPv4 or IPv6 address, optionally with a prefix length,
/// no wider than `/8` (IPv4) or `/16` (IPv6); see [`validate_allow_from_with`].
pub fn validate_allow_from(value: &str) -> Result<(), crate::error::SetupError> {
    validate_allow_from_with(value, false)
}

/// [`validate_allow_from`], optionally letting prefixes shorter than the minimum through
/// (`--allow-wide`).
///
/// Never accepted, flag or not: a zero-length prefix (it undoes the scoping entirely), a range
/// that contains the unspecified address (`0.0.0.0/8`, `0.0.0.0/1`, a bare `0.0.0.0` or `::`:
/// firewalls read these as "any"), and IPv4 multicast or broadcast space. Without the flag the
/// prefix must also be at least `/8` (IPv4) or `/16` (IPv6), because two halves such as
/// `0.0.0.0/1` plus `128.0.0.0/1` would together re-open every address.
pub fn validate_allow_from_with(value: &str, wide: bool) -> Result<(), crate::error::SetupError> {
    let bad = |why: &str| {
        tracing::warn!(value, why, "rejected --allow-from value");
        Err(crate::error::SetupError::BadAllowFrom {
            value: value.to_owned(),
            why: why.to_owned(),
        })
    };
    let (addr, prefix) = match value.split_once('/') {
        Some((a, p)) => (a, Some(p)),
        None => (value, None),
    };
    let Ok(ip) = addr.parse::<std::net::IpAddr>() else {
        return bad("not an IPv4 or IPv6 address");
    };
    let max: u8 = if ip.is_ipv4() { 32 } else { 128 };
    let prefix = match prefix {
        None => max,
        Some(p) => match p.parse::<u8>() {
            Ok(0) => return bad("a /0 prefix would admit every address"),
            Ok(n) if n <= max => n,
            _ => return bad("the prefix length is out of range"),
        },
    };
    let min = if ip.is_ipv4() {
        MIN_PREFIX_V4
    } else {
        MIN_PREFIX_V6
    };
    if prefix < min && !wide {
        return bad(&format!(
            "a /{prefix} range is wider than /{min}; narrow it, or pass --allow-wide if you really mean it"
        ));
    }
    match ip {
        std::net::IpAddr::V4(v4) => {
            let mask = u32::MAX.checked_shl(u32::from(32 - prefix)).unwrap_or(0);
            let net = u32::from(v4) & mask;
            if net >> 24 == 0 {
                return bad("the range contains 0.0.0.0, which a firewall reads as every address");
            }
            if net >> 28 >= 0xE {
                return bad("multicast and broadcast addresses are not remote computers");
            }
        }
        std::net::IpAddr::V6(v6) => {
            let mask = u128::MAX.checked_shl(u32::from(128 - prefix)).unwrap_or(0);
            if u128::from(v6) & mask == 0 {
                return bad("the range contains ::, which a firewall reads as every address");
            }
        }
    }
    Ok(())
}

/// Spec of the keepalive scheduled task resource.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskSpec {
    /// Distro the task keeps alive.
    pub distro: String,
    /// When it starts.
    pub keepalive: Keepalive,
    /// Free-text description stored on the task.
    pub description: String,
}

/// Accept only distro names that are safe inside a task command line and a PowerShell literal.
///
/// A leading `-` is refused too: the name follows `wsl.exe -d`, and `-d --shutdown` style names
/// would be read as options.
pub fn validate_distro(distro: &str) -> Result<(), crate::error::SetupError> {
    let ok = !distro.is_empty()
        && !distro.starts_with('-')
        && distro
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'));
    if ok {
        Ok(())
    } else {
        tracing::warn!(distro, "rejected distro name");
        Err(crate::error::SetupError::BadDistro(distro.to_owned()))
    }
}

/// The profile prefix of resource names: empty for the default profile.
fn prefix(profile: &str) -> String {
    if profile == DEFAULT_PROFILE {
        String::new()
    } else {
        format!("{profile} ")
    }
}

/// Display name of the inbound Windows Firewall rule (the default profile matches the hand-made rule).
pub fn firewall_rule_name(profile: &str, port: u16) -> String {
    format!("{}WSL SSH {port}", prefix(profile))
}

/// Name of the Hyper-V firewall rule.
pub fn hyperv_rule_name(profile: &str, port: u16) -> String {
    format!("{}WSL SSH {port} (Hyper-V)", prefix(profile))
}

/// Name of the keepalive scheduled task (the default logon task matches the hand-made one).
pub fn task_name(profile: &str, keepalive: Keepalive) -> String {
    format!(
        "{}WSL Keepalive{}",
        prefix(profile),
        keepalive_suffix(keepalive)
    )
}

/// The task-name suffix that tells the boot variant from the logon one.
fn keepalive_suffix(keepalive: Keepalive) -> &'static str {
    match keepalive {
        Keepalive::Logon => "",
        Keepalive::Boot => " (boot)",
    }
}

/// Name of the relay refresh task (named like the keepalive task, with `Relay` for `Keepalive`).
pub fn relay_task_name(profile: &str, keepalive: Keepalive) -> String {
    format!(
        "{}WSL Relay{}",
        prefix(profile),
        keepalive_suffix(keepalive)
    )
}

/// Path of the relay refresh script, inside the administrator-only directory.
pub fn relay_script_path(layout: &Layout) -> PathBuf {
    layout.admin_dir.join(relay::SCRIPT_NAME)
}

/// Path of the sshd drop-in that adds the listening port.
pub fn port_dropin_path(profile: &str) -> String {
    format!("{SSHD_DROPIN_DIR}/20-{profile}-port.conf")
}

/// Path of the sshd drop-in that disables password authentication.
pub fn hardening_dropin_path(profile: &str) -> String {
    format!("{SSHD_DROPIN_DIR}/10-{profile}-hardening.conf")
}

/// Contents of the port drop-in.
pub fn port_dropin(profile: &str, port: u16) -> String {
    format!(
        "# Managed by goway-setup (profile {profile}); removed by `goway-setup uninstall`.\nPort {port}\n"
    )
}

/// Contents of the hardening drop-in.
pub fn hardening_dropin(profile: &str) -> String {
    format!(
        "# Managed by goway-setup (profile {profile}); removed by `goway-setup uninstall`.\nPasswordAuthentication no\n"
    )
}

pub(crate) fn resource(kind: ResourceKind, name: String, spec: &impl Serialize) -> Change {
    Change::EnsureResource {
        kind,
        name,
        spec: serde_json::to_string(spec).expect("specs are plain structs and always serialize"),
    }
}

/// The changes of the host component, in application order (reverted in the opposite order).
///
/// Windows side: the machine-wide Add/Remove Programs entry that uninstalls the host from its
/// protected copy, `.wslconfig` mirrored networking, the inbound firewall rule (Private and Domain
/// profiles, local subnet plus `--allow-from` only), the Hyper-V
/// firewall rule (when the cmdlets exist) and the keepalive task. WSL side: systemd in
/// `/etc/wsl.conf`, the sshd package, the port (and optional hardening) drop-ins, and sshd
/// enabled at boot. The drop-ins are written before the package so a first install starts
/// listening on the right port.
pub fn host_plan(layout: &Layout, params: &HostParams, facts: &HostFacts) -> Vec<Change> {
    let profile = layout.profile.as_str();
    let port = params.port;
    let note = format!("goway-setup profile {profile}");
    // The Add/Remove Programs entry goes first so it is reverted last: if an uninstall stops
    // halfway, the entry that lets the user finish it is still there.
    let mut plan = vec![Change::EnsureRegKey {
        key: layout.host_uninstall_key.clone(),
    }];
    plan.extend(
        host_uninstall_values(layout, env!("CARGO_PKG_VERSION"))
            .into_iter()
            .map(|(name, value)| Change::SetRegistryValue {
                key: layout.host_uninstall_key.clone(),
                name: name.to_owned(),
                value,
            }),
    );
    if params.network == NetworkMode::Mirrored {
        plan.push(Change::SetIniKey {
            path: params.home.join(".wslconfig"),
            section: "wsl2".into(),
            key: "networkingMode".into(),
            value: "mirrored".into(),
        });
    }
    plan.extend([resource(
        ResourceKind::FirewallRule,
        firewall_rule_name(profile, port),
        &FirewallSpec {
            port,
            description: format!("WSL sshd; {note}"),
            scope: Scope::new(&params.allow_from),
        },
    )]);
    if facts.hyperv_firewall {
        plan.push(resource(
            ResourceKind::HyperVFirewallRule,
            hyperv_rule_name(profile, port),
            &HyperVSpec {
                port,
                vm_creator_id: WSL_VM_CREATOR_ID.into(),
                description: format!("WSL sshd; {note}"),
                scope: Scope::new(&params.allow_from),
            },
        ));
    }
    plan.push(resource(
        ResourceKind::ScheduledTask,
        task_name(profile, params.keepalive),
        &TaskSpec {
            distro: params.distro.clone(),
            keepalive: params.keepalive,
            description: format!(
                "keeps the {} WSL distro and its sshd running; {note}",
                params.distro
            ),
        },
    ));
    if params.network == NetworkMode::Nat {
        plan.extend(relay_changes(layout, params, &note));
    }
    plan.push(Change::SetIniKey {
        path: WSL_CONF.into(),
        section: "boot".into(),
        key: "systemd".into(),
        value: "true".into(),
    });
    plan.push(Change::EnsureDir {
        path: SSHD_DROPIN_DIR.into(),
    });
    if !facts.sshd_ports.contains(&port) {
        plan.push(Change::WriteFile {
            path: port_dropin_path(profile).into(),
            contents: port_dropin(profile, port),
        });
    }
    if params.harden && facts.authorized_keys {
        plan.push(Change::WriteFile {
            path: hardening_dropin_path(profile).into(),
            contents: hardening_dropin(profile),
        });
    }
    plan.push(Change::EnsureResource {
        kind: ResourceKind::WslPackage,
        name: SSHD_PACKAGE.into(),
        spec: String::new(),
    });
    plan.extend(SSHD_UNITS.iter().map(|unit| Change::EnsureResource {
        kind: ResourceKind::WslUnit,
        name: (*unit).into(),
        spec: String::new(),
    }));
    plan
}

/// The NAT-mode changes: the refresh script, the portproxy relay, and the task that keeps the
/// relay pointed at the distro. Applied in this order and reverted in the opposite one, so the
/// task never outlives the script it runs.
fn relay_changes(layout: &Layout, params: &HostParams, note: &str) -> Vec<Change> {
    let profile = layout.profile.as_str();
    let script = relay_script_path(layout);
    vec![
        Change::WriteFile {
            path: script.clone(),
            contents: relay::refresh_script(profile, &params.distro, params.port),
        },
        resource(
            ResourceKind::PortProxy,
            relay::relay_name(params.port),
            &PortProxySpec {
                listen_address: LISTEN_ADDRESS.to_owned(),
                port: params.port,
            },
        ),
        resource(
            ResourceKind::ScheduledTask,
            relay_task_name(profile, params.keepalive),
            &RelayTaskSpec {
                distro: params.distro.clone(),
                keepalive: params.keepalive,
                script: script.display().to_string(),
                interval_minutes: REFRESH_MINUTES,
                description: format!(
                    "points the port {} relay at the {} WSL distro's current address; {note}",
                    params.port, params.distro
                ),
            },
        ),
    ]
}

/// Whether `entry` really changed something (a no-op entry means the state pre-existed).
fn changed(entry: &Entry) -> bool {
    entry.prior != Prior::Noop && !entry.reverted
}

fn is_wsl_change(change: &Change) -> bool {
    match change {
        Change::WriteFile { path, .. }
        | Change::SetIniKey { path, .. }
        | Change::EnsureDir { path } => path.to_str().is_some_and(|s| s.starts_with('/')),
        Change::EnsureResource { kind, .. } => {
            matches!(kind, ResourceKind::WslPackage | ResourceKind::WslUnit)
        }
        _ => false,
    }
}

/// Whether the journal holds a live change to the distro's sshd (so a reload or restart is due).
pub fn sshd_changed(journal: &Journal) -> bool {
    journal
        .entries
        .iter()
        .any(|e| changed(e) && is_wsl_change(&e.change) && !is_wsl_conf(&e.change))
}

fn is_wsl_conf(change: &Change) -> bool {
    matches!(change, Change::SetIniKey { path, .. } if path.to_str() == Some(WSL_CONF))
}

/// The tasks this journal created (the keepalive, and in NAT mode the relay refresh), in plan
/// order: the ones worth starting now.
pub fn created_tasks(journal: &Journal) -> Vec<&str> {
    journal
        .entries
        .iter()
        .filter_map(|e| match &e.change {
            Change::EnsureResource {
                kind: ResourceKind::ScheduledTask,
                name,
                ..
            } if changed(e) => Some(name.as_str()),
            _ => None,
        })
        .collect()
}

/// The networking mode a host journal was built for: NAT exactly when it holds a relay.
pub fn network_in_journal(journal: &Journal) -> NetworkMode {
    let relay = journal.entries.iter().any(|e| {
        matches!(
            &e.change,
            Change::EnsureResource {
                kind: ResourceKind::PortProxy,
                ..
            }
        )
    });
    if relay {
        NetworkMode::Nat
    } else {
        NetworkMode::Mirrored
    }
}

/// Follow-ups the user must perform after a host install: settings only WSL's restart applies.
pub fn restart_notices(journal: &Journal) -> Vec<String> {
    let mut notices = Vec::new();
    for e in journal.entries.iter().filter(|e| changed(e)) {
        match &e.change {
            Change::SetIniKey { path, key, .. } if key == "networkingMode" => notices.push(format!(
                "{} changed; run `wsl --shutdown` (this ends all WSL sessions) for mirrored networking to apply",
                path.display()
            )),
            c @ Change::SetIniKey { .. } if is_wsl_conf(c) => notices.push(
                "/etc/wsl.conf changed; run `wsl --terminate <distro>` for it to apply".to_owned(),
            ),
            _ => {}
        }
    }
    notices
}

/// Which WSL restarts the install made necessary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RestartNeed {
    /// `.wslconfig` changed (mirrored networking): only `wsl --shutdown` applies it.
    pub shutdown: bool,
    /// `/etc/wsl.conf` changed: restarting the one distro applies it.
    pub terminate: bool,
}

impl RestartNeed {
    /// Whether no restart is needed.
    pub fn is_none(self) -> bool {
        !self.shutdown && !self.terminate
    }

    /// The exact command that applies the change (`wsl --shutdown` covers the distro restart too).
    pub fn command(self, distro: &str) -> Option<String> {
        if self.shutdown {
            Some("wsl --shutdown".to_owned())
        } else if self.terminate {
            Some(format!("wsl --terminate {distro}"))
        } else {
            None
        }
    }

    /// Plain words for what the restart does to the user's open Linux windows.
    pub fn consequence(self) -> &'static str {
        if self.shutdown {
            "This closes every open Linux (WSL) window on this laptop, so save your work in them first."
        } else {
            "This closes the open windows of that Linux distro, so save your work in them first."
        }
    }
}

/// The restarts the live entries of `journal` call for.
pub fn restart_need(journal: &Journal) -> RestartNeed {
    let mut need = RestartNeed::default();
    for e in journal.entries.iter().filter(|e| changed(e)) {
        match &e.change {
            Change::SetIniKey { key, .. } if key == "networkingMode" => need.shutdown = true,
            c @ Change::SetIniKey { .. } if is_wsl_conf(c) => need.terminate = true,
            _ => {}
        }
    }
    need
}

/// What to do about a needed restart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestartAction {
    /// Nothing to restart.
    Nothing,
    /// Ask on the console whether to restart now.
    Ask,
    /// Do not ask; print the exact command for the user to run when ready.
    PrintCommand,
}

/// Decide how to treat a needed restart: ask only on a console, never with `--yes` or
/// `--no-activate` (the latter is how live machines are protected from any restart).
pub fn restart_action(
    need: RestartNeed,
    yes: bool,
    activate: bool,
    console: bool,
) -> RestartAction {
    if need.is_none() {
        RestartAction::Nothing
    } else if yes || !activate || !console {
        RestartAction::PrintCommand
    } else {
        RestartAction::Ask
    }
}

/// Whether a console answer means yes; anything but y or yes (including empty) is no.
pub fn is_yes(answer: &str) -> bool {
    matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}

/// Whether the journal creates or removes the distro's port drop-in (the listening port changes).
pub fn dropin_in_journal(journal: &Journal, profile: &str) -> bool {
    let want = port_dropin_path(profile);
    journal.entries.iter().any(|e| {
        changed_or_reverted(e)
            && matches!(&e.change, Change::WriteFile { path, .. } if path.to_str() == Some(want.as_str()))
    })
}

fn changed_or_reverted(entry: &Entry) -> bool {
    entry.prior != Prior::Noop
}

/// Whether reverting the journal still changes firewall rules or tasks (needs administrator rights).
pub fn needs_admin(journal: &Journal) -> bool {
    journal.entries.iter().any(|e| {
        !e.reverted
            && e.prior != Prior::Noop
            && matches!(
                &e.change,
                Change::EnsureResource {
                    kind: ResourceKind::FirewallRule
                        | ResourceKind::HyperVFirewallRule
                        | ResourceKind::ScheduledTask
                        | ResourceKind::PortProxy
                        | ResourceKind::FirewallScope
                        | ResourceKind::WindowsCapability
                        | ResourceKind::Service,
                    ..
                }
            )
    })
}

/// Whether a resource name contains characters PowerShell cmdlets treat as wildcards.
pub fn has_wildcard(name: &str) -> bool {
    name.contains(['*', '?', '[', ']', '`'])
}

/// Every change the host plan can contain for these settings, over all optional steps
/// (Hyper-V rule or not, port drop-in or not, hardening or not, either keepalive).
///
/// The elevated uninstall accepts a journal entry only if it is in this set: a replay can then
/// never touch a path, registry key or resource name the install itself would not have.
pub fn expected_changes(
    layout: &Layout,
    settings: &HostSettings,
    home: &std::path::Path,
) -> Vec<Change> {
    if let Some(native) = &settings.native {
        return crate::native::expected_changes(layout, &settings.allow_from, native);
    }
    let mut all: Vec<Change> = Vec::new();
    // Only the mode the install recorded: a journal holding the other mode's entries is refused.
    let network = settings.network;
    for keepalive in [Keepalive::Logon, Keepalive::Boot] {
        for harden in [false, true] {
            for hyperv_firewall in [false, true] {
                for sshd_ports in [Vec::new(), vec![settings.port]] {
                    let params = HostParams {
                        port: settings.port,
                        distro: settings.distro.clone(),
                        keepalive,
                        harden,
                        allow_from: settings.allow_from.clone(),
                        home: home.to_path_buf(),
                        network,
                    };
                    let facts = HostFacts {
                        hyperv_firewall,
                        sshd_ports,
                        authorized_keys: true,
                        ..HostFacts::assumed()
                    };
                    for change in host_plan(layout, &params, &facts) {
                        for variant in [legacy_variant(&change), Some(change)]
                            .into_iter()
                            .flatten()
                        {
                            if !all.contains(&variant) {
                                all.push(variant);
                            }
                        }
                    }
                }
            }
        }
    }
    all
}

/// The firewall change as an install before scoping existed recorded it (no profile or
/// remote-address fields), so a journal from such an install can still be uninstalled.
fn legacy_variant(change: &Change) -> Option<Change> {
    let Change::EnsureResource { kind, name, spec } = change else {
        return None;
    };
    let legacy_spec = match kind {
        ResourceKind::FirewallRule => {
            let mut s: FirewallSpec = serde_json::from_str(spec).ok()?;
            s.scope = Scope::default();
            serde_json::to_string(&s).ok()?
        }
        ResourceKind::HyperVFirewallRule => {
            let mut s: HyperVSpec = serde_json::from_str(spec).ok()?;
            s.scope = Scope::default();
            serde_json::to_string(&s).ok()?
        }
        _ => return None,
    };
    Some(Change::EnsureResource {
        kind: *kind,
        name: name.clone(),
        spec: legacy_spec,
    })
}

/// The change with the recorded `DisplayVersion` of the host's Add/Remove Programs entry replaced
/// by this build's version, so an uninstaller of another version still accepts the entry; only a
/// short plain version string qualifies, anything else stays as recorded (and is then refused).
fn normalize_version(change: &Change, layout: &Layout) -> Change {
    if let Change::SetRegistryValue {
        key,
        name,
        value: RegValue::String(v),
    } = change
        && *key == layout.host_uninstall_key
        && name == "DisplayVersion"
        && v.len() <= 32
        && v.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+'))
    {
        return Change::SetRegistryValue {
            key: key.clone(),
            name: name.clone(),
            value: RegValue::String(env!("CARGO_PKG_VERSION").to_owned()),
        };
    }
    change.clone()
}

/// Most bytes of file text a journal prior may hold (a `.wslconfig` or a drop-in is tiny).
const MAX_PRIOR_BYTES: usize = 1 << 20;

/// Whether the prior state recorded for `change` is of the kind that change produces, and sane.
///
/// The elevated revert writes priors back with an administrator token, so a prior planted in the
/// journal must not smuggle in anything the change could not have captured: a different kind
/// of state, an oversized file, an "original line" that is several lines or not the changed
/// key, or a mode beyond the permission bits.
fn prior_fits(change: &Change, prior: &Prior) -> Result<(), String> {
    if *prior == Prior::Noop {
        return Ok(());
    }
    let kind_ok = matches!(
        (change, prior),
        (Change::WriteFile { .. }, Prior::File { .. })
            | (Change::EnsureLine { .. }, Prior::Line { .. })
            | (Change::EnsureDir { .. }, Prior::DirsCreated { .. })
            | (Change::InstallFile { .. }, Prior::FileInstalled)
            | (Change::EnsureRegKey { .. }, Prior::KeyCreated)
            | (Change::EnsureListEntry { .. }, Prior::ListEntry { .. })
            | (Change::SetRegistryValue { .. }, Prior::Registry { .. })
            | (
                Change::SetIniKey { .. },
                Prior::IniReplaced { .. } | Prior::IniInserted { .. }
            )
            | (Change::SetUnixMode { .. }, Prior::Mode { .. })
            | (Change::SetAcl { .. }, Prior::Acl { .. })
            | (Change::EnsureResource { .. }, Prior::ResourceCreated)
    );
    if !kind_ok {
        return Err(format!("prior {prior:?} is not what {change:?} records"));
    }
    match (change, prior) {
        (
            _,
            Prior::File {
                contents: Some(text),
            },
        ) if text.len() > MAX_PRIOR_BYTES => {
            Err("a recorded prior file is implausibly large".to_owned())
        }
        (Change::SetIniKey { key, .. }, Prior::IniReplaced { original_line }) => {
            let one_line = !original_line.contains(['\n', '\r', '\0']);
            let same_key = original_line
                .split_once('=')
                .is_some_and(|(k, _)| k.trim().eq_ignore_ascii_case(key));
            if one_line && same_key && original_line.len() <= 4096 {
                Ok(())
            } else {
                Err(format!(
                    "the recorded original line {original_line:?} is not one line setting {key}"
                ))
            }
        }
        (_, Prior::Mode { mode }) if *mode > 0o7777 => Err(format!(
            "the recorded mode {mode:o} is not a permission mode"
        )),
        _ => Ok(()),
    }
}

/// Refuse a host journal holding anything the host plan for `settings` could not have produced.
///
/// Run by the elevated uninstall before it reverts a single entry (see
/// [`crate::app::uninstall_checked`]). Besides membership in [`expected_changes`] it checks that
/// resource names carry no wildcard and that a directory-removal prior only lists ancestors of
/// the directory the entry ensured.
pub fn validate_journal(
    journal: &Journal,
    layout: &Layout,
    settings: &HostSettings,
    home: &std::path::Path,
) -> Result<(), crate::error::SetupError> {
    let allowed = expected_changes(layout, settings, home);
    let refuse = |index: usize, reason: String| {
        tracing::error!(index, %reason, "host journal entry refused");
        Err(crate::error::SetupError::UntrustedState {
            path: layout.host_journal_path.display().to_string(),
            reason: format!("entry {index}: {reason}"),
        })
    };
    for (index, entry) in journal.entries.iter().enumerate() {
        if let Change::EnsureResource { name, .. } = &entry.change
            && has_wildcard(name)
        {
            return refuse(
                index,
                format!("resource name {name:?} contains wildcard characters"),
            );
        }
        if !allowed.contains(&normalize_version(&entry.change, layout)) {
            return refuse(
                index,
                format!("{:?} is not a change the host install makes", entry.change),
            );
        }
        if let Err(reason) = prior_fits(&entry.change, &entry.prior) {
            return refuse(index, reason);
        }
        if let Prior::DirsCreated { created } = &entry.prior {
            let target = match &entry.change {
                Change::EnsureDir { path } => Some(path),
                _ => None,
            };
            let inside = |dir: &PathBuf| target.is_some_and(|t| t.starts_with(dir));
            if !created.iter().all(inside) {
                return refuse(
                    index,
                    "it would remove directories outside its own path".to_owned(),
                );
            }
        }
    }
    Ok(())
}
