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
