//! The values of the Add/Remove Programs (Uninstall) registry entry.

use goway_journal::RegValue;

use crate::layout::{DEFAULT_PROFILE, Layout};

/// Publisher shown in Add/Remove Programs.
const PUBLISHER: &str = "goway";

/// Display name: the product name, qualified by the profile when it is not the default.
pub fn display_name(profile: &str) -> String {
    if profile == DEFAULT_PROFILE {
        "goway".to_owned()
    } else {
        format!("goway ({profile})")
    }
}

/// The command line the Uninstall entry runs: the installed setup copy, quoted, plus its arguments.
pub fn uninstall_string(layout: &Layout) -> String {
    format!(
        "\"{}\" uninstall --profile {}",
        layout.setup_exe.display(),
        layout.profile
    )
}

/// All values written under the profile's Uninstall key, in write order.
pub fn uninstall_values(layout: &Layout, version: &str) -> Vec<(&'static str, RegValue)> {
    let s = |v: String| RegValue::String(v);
    vec![
        ("DisplayName", s(display_name(&layout.profile))),
        ("DisplayVersion", s(version.to_owned())),
        ("Publisher", s(PUBLISHER.to_owned())),
        (
            "InstallLocation",
            s(layout.install_root.display().to_string()),
        ),
        ("DisplayIcon", s(layout.goway_exe.display().to_string())),
        ("UninstallString", s(uninstall_string(layout))),
        ("NoModify", RegValue::Dword(1)),
        ("NoRepair", RegValue::Dword(1)),
    ]
}

/// Display name of the host component's entry: "goway helper (host)", qualified by a non-default profile.
pub fn host_display_name(profile: &str) -> String {
    if profile == DEFAULT_PROFILE {
        "goway helper (host)".to_owned()
    } else {
        format!("goway helper (host, profile {profile})")
    }
}

/// The command line the host's entry runs: the protected admin-dir copy, quoted, uninstalling the host only.
pub fn host_uninstall_string(layout: &Layout) -> String {
    format!(
        "\"{}\" uninstall --host --profile {}",
        crate::admin::protected_exe(&layout.admin_dir).display(),
        layout.profile
    )
}

/// All values written under the host component's machine-wide Uninstall key, in write order.
///
/// The entry runs the protected copy of goway-setup (never the user-writable download or client
/// copy), so removing the helper needs nothing the user has to keep.
pub fn host_uninstall_values(layout: &Layout, version: &str) -> Vec<(&'static str, RegValue)> {
    let s = |v: String| RegValue::String(v);
    let exe = crate::admin::protected_exe(&layout.admin_dir)
        .display()
        .to_string();
    vec![
        ("DisplayName", s(host_display_name(&layout.profile))),
        ("DisplayVersion", s(version.to_owned())),
        ("Publisher", s(PUBLISHER.to_owned())),
        ("InstallLocation", s(layout.admin_dir.display().to_string())),
        ("DisplayIcon", s(exe)),
        ("UninstallString", s(host_uninstall_string(layout))),
        ("NoModify", RegValue::Dword(1)),
        ("NoRepair", RegValue::Dword(1)),
    ]
}
