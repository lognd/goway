//! Finding a PowerShell for the tests that evaluate generated PowerShell source. They run
//! under pwsh on Linux and macOS and under either shell on Windows; without any PowerShell
//! they pass trivially (the CI jobs have one).
#![allow(dead_code)]

use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;

/// The PowerShell to drive: `GOWAY_PWSH`, else `pwsh`, else `powershell` on Windows.
pub fn pwsh() -> Option<&'static PathBuf> {
    static PS: OnceLock<Option<PathBuf>> = OnceLock::new();
    PS.get_or_init(|| {
        let mut candidates = Vec::new();
        if let Some(p) = std::env::var_os("GOWAY_PWSH") {
            candidates.push(PathBuf::from(p));
        }
        candidates.push(PathBuf::from("pwsh"));
        if cfg!(windows) {
            candidates.push(PathBuf::from("powershell"));
        }
        candidates.into_iter().find(|c| {
            Command::new(c)
                .args(["-NoProfile", "-Command", "exit 0"])
                .output()
                .is_ok_and(|o| o.status.success())
        })
    })
    .as_ref()
}
