//! The Windows side of this very machine, reached from WSL through
//! `powershell.exe` (WSL's interop): no ssh, no network listener, no key.
//!
//! The command goway sends is PowerShell source passed as
//! `-EncodedCommand`, so it reaches PowerShell byte for byte; arguments
//! are never re-split (see [`crate::transport::ps_call`]).

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::error::{Error, Result};

/// Environment variable naming the `powershell.exe` to use (for tests and
/// unusual installs); without it goway looks on `PATH` and in System32.
pub const POWERSHELL_ENV: &str = "GOWAY_POWERSHELL";

/// The environment an interop child keeps: what WSL needs to find the
/// Windows side, and nothing else (nothing of this shell's environment is
/// handed to Windows; `WSLENV` is emptied so nothing is shared either).
const KEPT_ENV: &[&str] = &["PATH", "WSL_INTEROP", "WSL_DISTRO_NAME"];

/// Whether this process runs inside WSL (Windows interop may exist).
pub fn is_wsl() -> bool {
    std::env::var_os("WSL_DISTRO_NAME").is_some()
        || std::fs::read_to_string("/proc/sys/kernel/osrelease")
            .is_ok_and(|s| s.to_ascii_lowercase().contains("microsoft"))
}

/// The `powershell.exe` to run: `GOWAY_POWERSHELL`, else (inside WSL) the
/// one on `PATH`, else the one in the usual System32 folder.
pub fn powershell() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os(POWERSHELL_ENV) {
        return Some(PathBuf::from(p));
    }
    if !is_wsl() {
        return None;
    }
    let on_path = std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|d| d.join("powershell.exe"))
            .find(|p| p.is_file())
    });
    on_path.or_else(|| {
        let p = PathBuf::from("/mnt/c/Windows/System32/WindowsPowerShell/v1.0/powershell.exe");
        p.is_file().then_some(p)
    })
}

/// A command running PowerShell `source` on the Windows side of this
/// machine. Fails when this is not WSL or `powershell.exe` is missing.
pub fn command(source: &str) -> Result<Command> {
    build(powershell(), source)
}

/// [`command`] with the `powershell.exe` already chosen (tests and tools
/// that know where it is).
pub fn command_with(exe: &Path, source: &str) -> Command {
    let mut cmd = Command::new(exe);
    cmd.args(crate::transport::POWERSHELL_FLAGS)
        .arg(crate::transport::encoded_command(source));
    cmd.env_clear();
    for name in KEPT_ENV {
        if let Some(v) = std::env::var_os(name) {
            cmd.env(name, v);
        }
    }
    cmd.env("WSLENV", "");
    tracing::debug!(exe = %exe.display(), "powershell interop");
    cmd
}

fn build(exe: Option<PathBuf>, source: &str) -> Result<Command> {
    let Some(exe) = exe else {
        return Err(Error::Usage(
            "the interop transport needs WSL with Windows interop (powershell.exe not found); \
             this host is for the Windows side of a WSL machine"
                .to_owned(),
        ));
    };
    Ok(command_with(&exe, source))
}

/// The Windows form of a WSL path (`wslpath -w`), when it has one.
pub fn to_windows_path(path: &Path) -> Option<String> {
    let out = Command::new("wslpath").arg("-w").arg(path).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim_end().to_owned())
        .filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    // frob:tests crates/goway/src/interop.rs::command
    #[test]
    fn command_is_powershell_with_an_encoded_script_and_a_scrubbed_environment() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("powershell.exe");
        std::fs::write(&exe, "#!/bin/sh\n").unwrap();
        let cmd = build(Some(exe.clone()), "Write-Output 'hi'").unwrap();
        assert!(build(None, "x").is_err());
        assert_eq!(cmd.get_program(), exe.as_os_str());
        let args: Vec<_> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            &args[..5],
            [
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-EncodedCommand"
            ]
        );
        assert_eq!(args.len(), 6);
        let leaked: Vec<_> = cmd
            .get_envs()
            .filter(|(k, v)| v.is_some() && !KEPT_ENV.iter().any(|n| k == n) && *k != "WSLENV")
            .collect();
        assert!(leaked.is_empty(), "{leaked:?}");
    }
}
