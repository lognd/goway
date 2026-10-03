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
}

impl HostFacts {
    /// What a dry run assumes on a fresh, capable machine.
    pub fn assumed() -> Self {
        Self {
            hyperv_firewall: true,
            sshd_ports: Vec::new(),
            authorized_keys: true,
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
}

impl Default for HostSettings {
    /// What an uninstall assumes when no settings were saved: the default distro and port.
    fn default() -> Self {
        Self {
            distro: DEFAULT_DISTRO.to_owned(),
            port: DEFAULT_PORT,
            allow_from: Vec::new(),
        }
    }
}

impl HostSettings {
    /// Refuse settings an elevated process must not build its expected plan from.
    pub fn validate(&self, path: &std::path::Path) -> Result<(), crate::error::SetupError> {
        validate_distro(&self.distro)?;
        for cidr in &self.allow_from {
            validate_allow_from(cidr)?;
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

/// Accept an `--allow-from` value: an IPv4 or IPv6 address, optionally with a prefix length.
///
/// A zero-length prefix (`0.0.0.0/0`) is refused because it would undo the scoping entirely.
pub fn validate_allow_from(value: &str) -> Result<(), crate::error::SetupError> {
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
    let max = if ip.is_ipv4() { 32 } else { 128 };
    if let Some(p) = prefix {
        match p.parse::<u8>() {
            Ok(0) => return bad("a /0 prefix would admit every address"),
            Ok(n) if n <= max => {}
            _ => return bad("the prefix length is out of range"),
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
pub fn validate_distro(distro: &str) -> Result<(), crate::error::SetupError> {
    let ok = !distro.is_empty()
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
    let suffix = match keepalive {
        Keepalive::Logon => "",
        Keepalive::Boot => " (boot)",
    };
    format!("{}WSL Keepalive{suffix}", prefix(profile))
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

fn resource(kind: ResourceKind, name: String, spec: &impl Serialize) -> Change {
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
    plan.extend([
        Change::SetIniKey {
            path: params.home.join(".wslconfig"),
            section: "wsl2".into(),
            key: "networkingMode".into(),
            value: "mirrored".into(),
        },
        resource(
            ResourceKind::FirewallRule,
            firewall_rule_name(profile, port),
            &FirewallSpec {
                port,
                description: format!("WSL sshd; {note}"),
                scope: Scope::new(&params.allow_from),
            },
        ),
    ]);
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

/// The keepalive task this journal created (the one worth starting now), if any.
pub fn created_task(journal: &Journal) -> Option<&str> {
    journal.entries.iter().find_map(|e| match &e.change {
        Change::EnsureResource {
            kind: ResourceKind::ScheduledTask,
            name,
            ..
        } if changed(e) => Some(name.as_str()),
        _ => None,
    })
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
                        | ResourceKind::ScheduledTask,
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
    let mut all: Vec<Change> = Vec::new();
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
                    };
                    let facts = HostFacts {
                        hyperv_firewall,
                        sshd_ports,
                        authorized_keys: true,
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
