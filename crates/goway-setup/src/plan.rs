//! Pure construction of the change plan for each installable component.

use std::path::PathBuf;

use goway_journal::{Change, ListPosition, RegValue};

use crate::entry::uninstall_values;
use crate::layout::Layout;

/// An installable part of goway; components are planned independently and concatenated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Component {
    /// goway.exe, the user PATH entry and the Add/Remove Programs entry (per-user, no admin).
    Client,
    /// Firewall rules, keepalive task, `.wslconfig` and the WSL sshd (see [`crate::host`]; needs administrator rights).
    Host,
}

/// Where the bytes to install come from, and what they hash to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sources {
    /// File holding the goway.exe payload to copy.
    pub goway_source: PathBuf,
    /// SHA-256 (hex) of the goway.exe payload.
    pub goway_digest: String,
    /// File holding this installer, copied so the uninstall entry has something to run.
    pub setup_source: PathBuf,
    /// SHA-256 (hex) of the installer.
    pub setup_digest: String,
}

/// The user environment variable holding the executable search path.
pub const PATH_VAR: &str = "Path";

/// The changes of the client component, in application order (reverted in the opposite order).
pub fn client_plan(layout: &Layout, src: &Sources, version: &str) -> Vec<Change> {
    let mut plan = vec![
        Change::EnsureDir {
            path: layout.bin_dir.clone(),
        },
        Change::InstallFile {
            path: layout.goway_exe.clone(),
            source: src.goway_source.clone(),
            digest: src.goway_digest.clone(),
        },
        Change::InstallFile {
            path: layout.setup_exe.clone(),
            source: src.setup_source.clone(),
            digest: src.setup_digest.clone(),
        },
        Change::EnsureListEntry {
            var: PATH_VAR.to_owned(),
            entry: layout.bin_dir.display().to_string(),
            separator: ';',
            position: ListPosition::Back,
        },
        Change::EnsureRegKey {
            key: layout.uninstall_key.clone(),
        },
    ];
    plan.extend(uninstall_values(layout, version).into_iter().map(
        |(name, value): (&str, RegValue)| Change::SetRegistryValue {
            key: layout.uninstall_key.clone(),
            name: name.to_owned(),
            value,
        },
    ));
    plan
}

/// The client plan when the client is selected.
///
/// The host plan needs probed facts and parameters, so it is built by [`crate::host::host_plan`]
/// and journaled separately (one journal per component).
pub fn build(
    layout: &Layout,
    components: &[Component],
    src: &Sources,
    version: &str,
) -> Vec<Change> {
    let mut sorted = components.to_vec();
    sorted.sort();
    sorted.dedup();
    sorted
        .into_iter()
        .flat_map(|c| match c {
            Component::Client => client_plan(layout, src, version),
            Component::Host => Vec::new(),
        })
        .collect()
}
