//! The Windows side of this very machine, reached from WSL through
//! `powershell.exe` (WSL's interop): no ssh, no network listener, no key.
//!
//! The command goway sends is PowerShell source passed as
//! `-EncodedCommand`, so it reaches PowerShell byte for byte; arguments
//! are never re-split (see [`crate::transport::ps_call`]).

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::error::{Error, Result};
use crate::remote::{self, Call};

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

/// Run `call` on the Windows side of this machine and return its stdout; a
/// missing remote script is installed first (once). `host` names the host in
/// errors.
///
/// # Errors
///
/// [`Error::Ssh`] when PowerShell cannot run, the call fails, or its answer
/// is larger than goway accepts.
pub fn call_probe(host: &str, call: &Call) -> Result<String> {
    let exe = powershell().ok_or_else(|| {
        Error::Usage(
            "the interop transport needs WSL with Windows interop (powershell.exe not found)"
                .to_owned(),
        )
    })?;
    call_probe_via(&exe, host, call)
}

/// [`call_probe`] with the `powershell.exe` already chosen.
pub fn call_probe_via(exe: &Path, host: &str, call: &Call) -> Result<String> {
    let fail = |message: String| Error::Ssh {
        host: host.to_owned(),
        message,
    };
    let run = |source: &str| -> Result<Vec<u8>> {
        crate::sync::exchange_child(command_with(exe, source), b"").map_err(|e| match e {
            Error::Ssh { message, .. } => fail(message),
            other => other,
        })
    };
    let mut out = run(&call.powershell_probe())?;
    if String::from_utf8_lossy(&out).trim() == remote::PROBE_NEEDS_INSTALL {
        tracing::info!(host, "installing the remote script on the Windows side");
        crate::sync::exchange_child(
            command_with(exe, &remote::ps_install()),
            remote::SCRIPT_PS.as_bytes(),
        )
        .map_err(|e| fail(e.to_string()))?;
        out = run(&call.powershell_probe())?;
    }
    Ok(String::from_utf8_lossy(&out).into_owned())
}

/// The Windows form of a WSL path (`wslpath -w`), when it has one.
pub fn to_windows_path(path: &Path) -> Option<String> {
    let out = Command::new("wslpath").arg("-w").arg(path).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim_end().to_owned())
        .filter(|s| !s.is_empty())
}

/// The WSL path of a Windows path (`wslpath -u`), when it has one.
pub fn to_wsl_path(windows: &str) -> Option<PathBuf> {
    let out = Command::new("wslpath")
        .arg("-u")
        .arg(windows)
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim_end().to_owned())
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
}

/// `path` with `.` and `..` resolved by text only (no filesystem access, so
/// nothing is ever stat-ed across the slow drvfs boundary).
fn normalized(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// `path` normalized, when it lies inside `root`; goway never reads or
/// writes anything on the Windows side outside its own directory.
///
/// # Errors
///
/// [`Error::Usage`] when `path` is not inside `root`.
pub fn within_root(root: &Path, path: &Path) -> Result<PathBuf> {
    let root = normalized(root);
    let path = normalized(path);
    if path.starts_with(&root) && path != root {
        Ok(path)
    } else {
        tracing::warn!(root = %root.display(), path = %path.display(), "path outside goway's root refused");
        Err(Error::Usage(format!(
            "{} is outside goway's directory {}; refusing to touch it",
            path.display(),
            root.display()
        )))
    }
}

/// Where goway's root directory is on the Windows side (an absolute Windows
/// path), asked of PowerShell so the home directory is never guessed.
///
/// # Errors
///
/// [`Error::Usage`] when PowerShell cannot run or answers nothing.
pub fn windows_root(remote_root: &str) -> Result<String> {
    let source = format!(
        "$r = {}; if (-not [IO.Path]::IsPathRooted($r)) {{ $r = [IO.Path]::Combine([Environment]::GetFolderPath('UserProfile'), $r) }}; [Console]::Out.Write([IO.Path]::GetFullPath($r))",
        crate::transport::ps_quote(remote_root)
    );
    let out = command(&source)?
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| Error::Usage(format!("cannot run powershell.exe: {e}")))?;
    let text = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    if !out.status.success() || text.is_empty() {
        return Err(Error::Usage(
            "powershell.exe did not report goway's directory".to_owned(),
        ));
    }
    Ok(text)
}

/// The WSL view of `parts` under goway's directory on the Windows side
/// (for showing a kept work dir): converted with `wslpath`, and refused when
/// it would leave the root.
///
/// # Errors
///
/// [`Error::Usage`] when the root cannot be found, converted, or `parts`
/// leave it.
pub fn wsl_path_under_root(remote_root: &str, parts: &[&str]) -> Result<PathBuf> {
    let root_win = windows_root(remote_root)?;
    let root = to_wsl_path(&root_win)
        .ok_or_else(|| Error::Usage(format!("wslpath cannot convert {root_win}")))?;
    let mut path = root.clone();
    for p in parts {
        path.push(p);
    }
    // Inside the root's own directory only: the root itself is not a target.
    within_root(&root, &path)
}

#[cfg(test)]
mod tests {
    use super::*;

    // frob:tests crates/goway/src/interop.rs::within_root
    #[test]
    fn paths_outside_goways_root_are_refused() {
        let root = Path::new("/mnt/c/Users/u/.cache/goway");
        assert!(within_root(root, &root.join("work/r1/tree")).is_ok());
        assert!(within_root(root, &root.join("work/../../elsewhere")).is_err());
        assert!(within_root(root, Path::new("/mnt/c/Users/u/.cache/goway-other/x")).is_err());
        assert!(within_root(root, Path::new("/mnt/c/Windows")).is_err());
        assert!(within_root(root, root).is_err());
        assert_eq!(
            within_root(root, &root.join("work/./a/../b")).unwrap(),
            root.join("work/b")
        );
    }

    // frob:tests crates/goway/src/interop.rs::to_wsl_path
    #[test]
    fn windows_paths_convert_with_wslpath_when_it_exists() {
        if Command::new("wslpath")
            .arg("-u")
            .arg("C:\\")
            .output()
            .is_err()
        {
            return;
        }
        assert_eq!(
            to_wsl_path("C:\\Users").as_deref(),
            Some(Path::new("/mnt/c/Users"))
        );
    }

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

    // frob:tests crates/goway/src/interop.rs::call_probe
    #[cfg(unix)]
    #[test]
    fn a_missing_script_is_installed_and_the_probe_repeated() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("powershell.exe");
        // Call 1 says "needs install", call 2 (the install) reads its stdin,
        // call 3 answers the probe.
        let script = format!(
            "#!/bin/sh\nn=$(cat '{d}/n' 2>/dev/null || echo 0); n=$((n+1)); echo $n > '{d}/n'\n\
             case $n in 1) echo {mark};; 2) cat >/dev/null;; *) printf 'arch=aarch64\\n';; esac\n",
            d = dir.path().display(),
            mark = remote::PROBE_NEEDS_INSTALL
        );
        std::fs::write(&exe, script).unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        let out = call_probe_via(&exe, "winbox", &Call::new("probe", &["root"])).unwrap();
        assert_eq!(out, "arch=aarch64\n");
        assert_eq!(
            std::fs::read_to_string(dir.path().join("n"))
                .unwrap()
                .trim(),
            "3"
        );
    }
}
