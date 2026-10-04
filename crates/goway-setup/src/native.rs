//! The native host component: a Windows machine without WSL becomes a goway helper through
//! Windows' own OpenSSH Server.
//!
//! Pure construction only (no probing, no side effects), like [`crate::host`]: the plan is built
//! from probed [`NativeFacts`] and tested against the model system and a fake runner. Every
//! change uses the ordinary `Change` vocabulary, so `uninstall` replays the journal backwards:
//!
//! * the OpenSSH Server capability is a resource that exists while installed, so a capability
//!   the user already had records nothing and is never removed;
//! * sshd's startup type and running state are probed first and carried in the resource's spec,
//!   so removal puts back exactly what was there;
//! * the built-in firewall rule the capability creates (every profile, every address) is
//!   narrowed to local networks, but only when this install caused it to exist;
//! * the authorized key is one journaled line plus its ACL, so keys the user had stay.

use std::path::{Path, PathBuf};

use goway_journal::{Change, RegValue, ResourceKind, sha256_hex};
use serde::{Deserialize, Serialize};

use crate::entry::host_uninstall_values;
use crate::error::SetupError;
use crate::host::{self, FirewallSpec, Scope};
use crate::layout::Layout;

/// The port Windows' sshd listens on; the native install does not move it.
pub const NATIVE_PORT: u16 = 22;
/// The optional capability that provides sshd.
pub const CAPABILITY: &str = "OpenSSH.Server~~~~0.0.1";
/// The Windows service name of sshd.
pub const SSHD_SERVICE: &str = "sshd";
/// The `Name` of the firewall rule the capability creates (open to every address and profile).
pub const BUILTIN_RULE: &str = "OpenSSH-Server-In-TCP";
/// Registry key where sshd reads its default shell.
pub const OPENSSH_KEY: &str = r"HKLM\SOFTWARE\OpenSSH";
/// The value naming the program sshd starts for every login.
pub const SHELL_VALUE: &str = "DefaultShell";
/// File (in `%ProgramData%\ssh`) holding the authorized keys of every administrator account.
pub const ADMIN_KEYS_FILE: &str = "administrators_authorized_keys";
/// Name of sshd's ed25519 host public key (in `%ProgramData%\ssh`).
pub const HOST_KEY_FILE: &str = "ssh_host_ed25519_key.pub";
/// DACL of the administrators' key file: SYSTEM and Administrators only, no inheritance.
pub const ADMIN_KEYS_SDDL: &str = "D:P(A;;FA;;;SY)(A;;FA;;;BA)";

/// A service startup type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StartType {
    /// Starts at boot.
    Automatic,
    /// Starts on demand.
    Manual,
    /// Cannot start.
    Disabled,
}

impl StartType {
    /// The name `Set-Service -StartupType` takes.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Automatic => "Automatic",
            Self::Manual => "Manual",
            Self::Disabled => "Disabled",
        }
    }

    /// Parse what the probe prints (`Auto`, `Automatic`, `Manual`, `Disabled`).
    pub fn parse(text: &str) -> Option<Self> {
        match text.trim().to_ascii_lowercase().as_str() {
            "auto" | "automatic" => Some(Self::Automatic),
            "manual" => Some(Self::Manual),
            "disabled" => Some(Self::Disabled),
            _ => None,
        }
    }
}

/// What to restore when the sshd service resource is removed: the service ends up automatic and
/// running; these are what it was before. The resource's removal is given only its name, so the
/// name carries them (`sshd:Manual:stopped`), which also makes the journal entry self-describing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceSpec {
    /// The startup type before the install.
    pub restore: StartType,
    /// Whether sshd was running before the install.
    pub was_running: bool,
}

impl ServiceSpec {
    /// The resource name that encodes this spec.
    pub fn resource_name(&self) -> String {
        format!(
            "{SSHD_SERVICE}:{}:{}",
            self.restore.as_str(),
            if self.was_running {
                "running"
            } else {
                "stopped"
            }
        )
    }

    /// Decode a resource name made by [`Self::resource_name`]; `None` for anything else.
    pub fn from_resource_name(name: &str) -> Option<Self> {
        let mut parts = name.split(':');
        let (service, start, state) = (parts.next()?, parts.next()?, parts.next()?);
        if service != SSHD_SERVICE || parts.next().is_some() {
            return None;
        }
        let restore = match start {
            "Automatic" => StartType::Automatic,
            "Manual" => StartType::Manual,
            "Disabled" => StartType::Disabled,
            _ => return None,
        };
        let was_running = match state {
            "running" => true,
            "stopped" => false,
            _ => return None,
        };
        Some(Self {
            restore,
            was_running,
        })
    }
}

/// Spec of the narrowed built-in firewall rule: the profiles and addresses it is limited to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScopeSpec {
    /// Profiles and remote addresses the rule is limited to.
    #[serde(flatten)]
    pub scope: Scope,
}

/// The account that will log in over ssh (the one running the install) and where its key goes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyAccount {
    /// `DOMAIN\user` as Windows names it.
    pub name: String,
    /// The account's SID (`S-1-5-21-...`).
    pub sid: String,
    /// Whether the account belongs to Administrators: sshd then reads its keys from
    /// `administrators_authorized_keys`, not from the profile.
    pub admin: bool,
    /// The account's profile directory.
    pub profile_dir: PathBuf,
}

impl KeyAccount {
    /// The file sshd reads this account's keys from.
    pub fn keys_file(&self, program_data: &Path) -> PathBuf {
        if self.admin {
            program_data.join("ssh").join(ADMIN_KEYS_FILE)
        } else {
            self.profile_dir.join(".ssh").join("authorized_keys")
        }
    }

    /// The DACL of that file: SYSTEM and Administrators for administrators; the user and SYSTEM
    /// (sshd's own account) for everyone else.
    pub fn keys_sddl(&self) -> String {
        if self.admin {
            ADMIN_KEYS_SDDL.to_owned()
        } else {
            format!("D:P(A;;FA;;;{})(A;;FA;;;SY)", self.sid)
        }
    }

    /// Refuse values an elevated process must not build its expected plan from.
    pub fn validate(&self) -> Result<(), String> {
        let sid_ok = self.sid.starts_with("S-1-")
            && self.sid.len() <= 100
            && self
                .sid
                .chars()
                .all(|c| c.is_ascii_digit() || c == 'S' || c == '-');
        if !sid_ok {
            return Err(format!("{:?} is not a SID", self.sid));
        }
        let text = self.profile_dir.to_string_lossy();
        // Windows paths are judged as text, so the check is the same wherever it runs.
        let b = text.as_bytes();
        let absolute = text.starts_with('/')
            || text.starts_with("\\\\")
            || (b.len() > 2
                && b[0].is_ascii_alphabetic()
                && b[1] == b':'
                && matches!(b[2], b'\\' | b'/'));
        let dir_ok = absolute
            && !text.contains(['\n', '\r', '\0'])
            && text.split(['\\', '/']).all(|part| part != "..");
        if !dir_ok {
            return Err(format!(
                "{text:?} is not a plain absolute profile directory"
            ));
        }
        if self.name.is_empty() || self.name.contains(['\n', '\r', '\0']) {
            return Err("the account name is empty or has control characters".to_owned());
        }
        Ok(())
    }
}

/// What the user asked the native install to set up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeParams {
    /// Extra remote addresses (validated CIDRs) allowed in besides the local subnet.
    pub allow_from: Vec<String>,
    /// The main laptop's public key to authorize, if given.
    pub authorized_key: Option<String>,
    /// The account that logs in.
    pub account: KeyAccount,
    /// Absolute path of the PowerShell sshd starts for every login.
    pub shell: String,
}

/// How the built-in firewall rule currently admits connections.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltinRule {
    /// There is no such rule.
    Absent,
    /// It admits only Private and Domain networks and the local subnet (or narrower).
    Scoped,
    /// It admits Public networks or every address.
    Open,
}

/// What probing the machine found; it decides which optional steps the plan contains.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeFacts {
    /// Whether the OpenSSH Server capability is installed.
    pub capability_installed: bool,
    /// sshd's startup type and running state, when the service exists.
    pub service: Option<(StartType, bool)>,
    /// `DefaultShell` as set now, if any.
    pub default_shell: Option<String>,
    /// How the built-in firewall rule admits connections now.
    pub builtin_rule: BuiltinRule,
}

impl NativeFacts {
    /// What a dry run assumes on a fresh machine (nothing of sshd present).
    pub fn assumed() -> Self {
        Self {
            capability_installed: false,
            service: None,
            default_shell: None,
            builtin_rule: BuiltinRule::Absent,
        }
    }
}

/// Settings persisted next to the host journal for a native install (see [`crate::host::HostSettings`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeSettings {
    /// The key the install authorized, if any.
    #[serde(default)]
    pub authorized_key: Option<String>,
    /// The account the key was installed for.
    pub account: KeyAccount,
}

impl NativeSettings {
    /// Refuse settings an elevated uninstall must not build its expected plan from.
    pub fn validate(&self, path: &Path) -> Result<(), SetupError> {
        let untrusted = |reason: String| SetupError::UntrustedState {
            path: path.display().to_string(),
            reason,
        };
        self.account.validate().map_err(untrusted)?;
        if let Some(key) = &self.authorized_key {
            validate_public_key(key).map_err(|e| untrusted(e.to_string()))?;
        }
        Ok(())
    }
}

/// The name of the firewall rule goway adds for sshd.
pub fn firewall_rule_name(profile: &str) -> String {
    format!(
        "{}OpenSSH SSH {NATIVE_PORT}",
        if profile == crate::layout::DEFAULT_PROFILE {
            String::new()
        } else {
            format!("{profile} ")
        }
    )
}

/// The marker that tags goway's line in an authorized-keys file.
pub fn key_marker(profile: &str) -> String {
    format!("# goway-setup {profile}")
}

/// `%ProgramData%` as the layout knows it (the parent of goway's administrator-only root).
pub fn program_data(layout: &Layout) -> PathBuf {
    layout
        .admin_root
        .parent()
        .map_or_else(|| layout.admin_root.clone(), Path::to_path_buf)
}

/// Where sshd keeps its host public key.
pub fn host_key_path(layout: &Layout) -> PathBuf {
    program_data(layout).join("ssh").join(HOST_KEY_FILE)
}

const KEY_TYPES: [&str; 6] = [
    "ssh-ed25519",
    "ecdsa-sha2-nistp256",
    "ecdsa-sha2-nistp384",
    "ecdsa-sha2-nistp521",
    "ssh-rsa",
    "sk-ssh-ed25519@openssh.com",
];

/// Standard base64 (padding optional) to bytes; `None` for anything else.
pub fn base64_decode(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    let mut acc = 0u32;
    let mut bits = 0u32;
    for c in text.trim_end_matches('=').bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        };
        acc = (acc << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(u8::try_from((acc >> bits) & 0xff).ok()?);
        }
    }
    Some(out)
}

/// Accept one ssh public key line: a known key type, a body that decodes and names that same
/// type, no leading options and no control characters. Only such a line is ever written to an
/// authorized-keys file.
pub fn validate_public_key(line: &str) -> Result<(), SetupError> {
    let bad = |why: &str| {
        tracing::warn!(why, "rejected authorized key");
        Err(SetupError::BadAuthorizedKey(why.to_owned()))
    };
    if line.contains(['\n', '\r', '\0']) || line.chars().any(char::is_control) {
        return bad("it is not a single line of plain text");
    }
    let mut parts = line.split_whitespace();
    let (Some(kind), Some(body)) = (parts.next(), parts.next()) else {
        return bad("it needs a key type and a key");
    };
    if !KEY_TYPES.contains(&kind) {
        return bad(
            "the key type is not one goway authorizes (options before the type are not allowed)",
        );
    }
    let Some(blob) = base64_decode(body) else {
        return bad("the key is not valid base64");
    };
    let named = blob
        .get(..4)
        .and_then(|n| <[u8; 4]>::try_from(n).ok())
        .map(u32::from_be_bytes)
        .and_then(|n| blob.get(4..4 + usize::try_from(n).ok()?));
    if named != Some(kind.as_bytes()) {
        return bad("the key does not match its stated type");
    }
    Ok(())
}

/// The `SHA256:...` fingerprint of an ssh public key line (as `ssh-keygen -lf` prints it);
/// `None` when the line is not a key.
pub fn fingerprint(public_key: &str) -> Option<String> {
    let body = public_key.split_whitespace().nth(1)?;
    let blob = base64_decode(body)?;
    let digest: Vec<u8> = sha256_hex(&blob)
        .as_bytes()
        .chunks(2)
        .filter_map(|p| u8::from_str_radix(std::str::from_utf8(p).ok()?, 16).ok())
        .collect();
    Some(format!(
        "SHA256:{}",
        crate::ps::base64(&digest).trim_end_matches('=')
    ))
}

/// The authorized key given as a line, or read from a `.pub` file's text.
pub fn key_from_argument(
    argument: &str,
    read: impl FnOnce(&Path) -> Option<String>,
) -> Result<String, SetupError> {
    let is_line = KEY_TYPES
        .iter()
        .any(|t| argument.starts_with(&format!("{t} ")));
    let text = if is_line {
        argument.to_owned()
    } else {
        read(Path::new(argument)).ok_or_else(|| {
            SetupError::BadAuthorizedKey(format!(
                "{argument:?} is neither a public key line nor a readable file"
            ))
        })?
    };
    let line = text
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("")
        .to_owned();
    validate_public_key(&line)?;
    Ok(line)
}

/// The changes of the native host component, in application order (reverted in the opposite one).
///
/// The Add/Remove Programs entry goes first so it is reverted last. The capability comes before
/// everything that needs sshd's files; the service goes last so sshd starts with the shell, the
/// key and the firewall already in place.
pub fn native_plan(layout: &Layout, params: &NativeParams, facts: &NativeFacts) -> Vec<Change> {
    let profile = layout.profile.as_str();
    let note = format!("goway-setup profile {profile}");
    let scope = Scope::new(&params.allow_from);
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
    plan.push(Change::EnsureResource {
        kind: ResourceKind::WindowsCapability,
        name: CAPABILITY.to_owned(),
        spec: String::new(),
    });
    // The capability's own rule admits every network; narrow it when this install makes it.
    // A capability that was already there brought a rule that is the user's to manage.
    if !facts.capability_installed {
        plan.push(host::resource(
            ResourceKind::FirewallScope,
            BUILTIN_RULE.to_owned(),
            &ScopeSpec {
                scope: scope.clone(),
            },
        ));
    }
    plan.push(host::resource(
        ResourceKind::FirewallRule,
        firewall_rule_name(profile),
        &FirewallSpec {
            port: NATIVE_PORT,
            description: format!("OpenSSH Server; {note}"),
            scope,
        },
    ));
    plan.extend([
        Change::EnsureRegKey {
            key: OPENSSH_KEY.to_owned(),
        },
        Change::SetRegistryValue {
            key: OPENSSH_KEY.to_owned(),
            name: SHELL_VALUE.to_owned(),
            value: RegValue::String(params.shell.clone()),
        },
    ]);
    if let Some(key) = &params.authorized_key {
        let file = params.account.keys_file(&program_data(layout));
        if let Some(dir) = file.parent() {
            plan.push(Change::EnsureDir {
                path: dir.to_path_buf(),
            });
        }
        plan.push(Change::EnsureLine {
            path: file.clone(),
            line: key.clone(),
            marker: key_marker(profile),
        });
        plan.push(Change::SetAcl {
            path: file,
            sddl: params.account.keys_sddl(),
        });
    }
    let (restore, was_running) = facts.service.unwrap_or((StartType::Manual, false));
    plan.push(Change::EnsureResource {
        kind: ResourceKind::Service,
        name: ServiceSpec {
            restore,
            was_running,
        }
        .resource_name(),
        spec: String::new(),
    });
    plan
}

/// Every change the native plan can contain for these settings, over all optional steps
/// (capability present or not, any sshd state, any `DefaultShell` before): the elevated
/// uninstall accepts a journal entry only if it is in this set.
pub fn expected_changes(
    layout: &Layout,
    allow_from: &[String],
    native: &NativeSettings,
) -> Vec<Change> {
    let mut all: Vec<Change> = Vec::new();
    let params = NativeParams {
        allow_from: allow_from.to_vec(),
        authorized_key: native.authorized_key.clone(),
        account: native.account.clone(),
        shell: crate::sysapi::tool_path(crate::sysapi::Tool::PowerShell),
    };
    let states = [StartType::Automatic, StartType::Manual, StartType::Disabled]
        .into_iter()
        .flat_map(|s| [(s, true), (s, false)]);
    for capability_installed in [false, true] {
        for service in states.clone().map(Some).chain([None]) {
            let facts = NativeFacts {
                capability_installed,
                service,
                default_shell: None,
                builtin_rule: BuiltinRule::Absent,
            };
            for change in native_plan(layout, &params, &facts) {
                if !all.contains(&change) {
                    all.push(change);
                }
            }
        }
    }
    all
}

/// What the install has to tell the user about the built-in firewall rule it left alone.
pub fn open_rule_warning(facts: &NativeFacts) -> Option<String> {
    (facts.capability_installed && facts.builtin_rule == BuiltinRule::Open).then(|| {
        format!(
            "the existing firewall rule {BUILTIN_RULE} admits every address and Public networks, so sshd is reachable beyond your local network. \
             goway did not change a rule it did not create. To limit it yourself, open PowerShell as administrator and run: \
             Set-NetFirewallRule -Name {BUILTIN_RULE} -Profile Private,Domain -RemoteAddress LocalSubnet"
        )
    })
}

/// What the install has to tell the user about a `DefaultShell` it replaces.
pub fn shell_replaced_notice(facts: &NativeFacts, shell: &str) -> Option<String> {
    facts
        .default_shell
        .as_deref()
        .filter(|old| !old.eq_ignore_ascii_case(shell))
        .map(|old| {
            format!(
                "sshd's default shell was {old} and is now PowerShell for every ssh login on this machine; uninstall puts {old} back"
            )
        })
}
