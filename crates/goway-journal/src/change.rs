//! The vocabulary of desired changes.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// A typed registry-like value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum RegValue {
    /// A plain string (`REG_SZ`).
    String(String),
    /// A string with unexpanded variables (`REG_EXPAND_SZ`).
    ExpandString(String),
    /// A 32-bit integer (`REG_DWORD`).
    Dword(u32),
}

/// The kind of a named external resource.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceKind {
    /// A Windows Defender Firewall rule.
    FirewallRule,
    /// A Hyper-V firewall rule.
    HyperVFirewallRule,
    /// A scheduled task.
    ScheduledTask,
    /// A Windows `netsh interface portproxy` IPv4 relay; `name` is `<listen address>:<port>`.
    PortProxy,
    /// A system service: present while it is running and starts automatically. The creation spec
    /// carries the startup type and running state to restore, so removal is exact.
    Service,
    /// A Windows optional capability (such as the OpenSSH Server), present while installed;
    /// a capability that was already installed is never recorded, so it is never removed.
    WindowsCapability,
    /// The scope (profiles and remote addresses) of a firewall rule that the system created
    /// itself, looked up by rule name; removal restores the rule to every profile and address.
    FirewallScope,
    /// A distro package (exists while installed); managed inside a WSL distro.
    WslPackage,
    /// A systemd unit enabled at boot (exists while enabled); managed inside a WSL distro.
    WslUnit,
    /// An ssh key pair: `name` is the private key path (the public key is `name.pub`),
    /// the spec is the key comment. Created with `ssh-keygen`; private key contents are
    /// never read or journaled.
    SshKeyPair,
}

/// Where a new list entry goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ListPosition {
    /// Before all existing entries.
    Front,
    /// After all existing entries.
    Back,
}

/// One desired change; applying it records the prior state so it can be inverted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "change", rename_all = "snake_case")]
pub enum Change {
    /// Make a file hold exactly `contents`.
    WriteFile {
        /// File path.
        path: PathBuf,
        /// Desired contents.
        contents: String,
    },
    /// Ensure the line `<line> <marker>` exists in a text file.
    EnsureLine {
        /// File path (created if absent).
        path: PathBuf,
        /// The line text without the marker.
        line: String,
        /// Marker comment appended to the line; identifies goway's line.
        marker: String,
    },
    /// Ensure a directory (and its missing ancestors) exists.
    EnsureDir {
        /// Directory path.
        path: PathBuf,
    },
    /// Ensure an entry is present in a PATH-like list variable.
    EnsureListEntry {
        /// Variable name.
        var: String,
        /// The entry to add.
        entry: String,
        /// The list separator, for example `;` or `:`.
        separator: char,
        /// Where to insert when the entry is absent.
        position: ListPosition,
    },
    /// Make a file a byte-exact copy of `source`, which must hold the content with `digest`.
    InstallFile {
        /// Destination path (its parent must exist; an existing different file is refused).
        path: PathBuf,
        /// Path of the file to copy from, for example a staged payload.
        source: PathBuf,
        /// Lowercase hex SHA-256 of the content; revert removes the file only while it still matches.
        digest: String,
    },
    /// Ensure a registry-like key exists (values are separate changes).
    EnsureRegKey {
        /// Key path.
        key: String,
    },
    /// Set a registry-like value.
    SetRegistryValue {
        /// Key path.
        key: String,
        /// Value name.
        name: String,
        /// Desired value.
        value: RegValue,
    },
    /// Set `key=value` inside `[section]` of an ini file.
    SetIniKey {
        /// Ini file path (created if absent).
        path: PathBuf,
        /// Section name without brackets.
        section: String,
        /// Key name.
        key: String,
        /// Desired value.
        value: String,
    },
    /// Set a unix file mode.
    SetUnixMode {
        /// Target path.
        path: PathBuf,
        /// Desired permission bits.
        mode: u32,
    },
    /// Set a Windows ACL given as an SDDL string.
    SetAcl {
        /// Target path.
        path: PathBuf,
        /// Desired SDDL.
        sddl: String,
    },
    /// Ensure a named external resource exists.
    EnsureResource {
        /// Resource kind.
        kind: ResourceKind,
        /// Resource name.
        name: String,
        /// Opaque creation spec, interpreted by the `System` implementation.
        spec: String,
    },
}
