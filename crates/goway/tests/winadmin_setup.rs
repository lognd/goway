//! Audit 3, H2: the administrator routes must never start `goway-setup` by name. The step
//! script is run under a real PowerShell with the ACL lookup (`Get-Acl`) replaced by a model,
//! on a machine whose PATH starts with a planted `goway-setup`: the planted program never
//! runs, a copy in a protected place is run by absolute path with its arguments intact, and a
//! copy whose owner, file or directory a normal user could have written is refused.
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use goway::winadmin::{EXIT_NO_TRUSTED_SETUP, WinStep};

mod pwsh;

/// Replaces `Get-Acl`: every path is owned by Administrators and writable only by them, except
/// where `MOCK_OWNER` names a different owner for paths containing `MOCK_MATCH`, or
/// `MOCK_WRITABLE` gives Users write access to paths containing `MOCK_MATCH`.
const MOCK_ACL: &str = r"
function Get-Acl {
    param([string]$LiteralPath)
    $hit = $env:MOCK_MATCH -and $LiteralPath.Contains($env:MOCK_MATCH)
    $owner = 'S-1-5-32-544'
    if ($hit -and $env:MOCK_OWNER) { $owner = $env:MOCK_OWNER }
    $aces = @(
        @{ Sid = 'S-1-5-32-544'; Rights = 'FullControl' },
        @{ Sid = 'S-1-5-32-545'; Rights = 'ReadAndExecute' }
    )
    if ($hit -and $env:MOCK_WRITABLE) { $aces += @{ Sid = 'S-1-5-32-545'; Rights = $env:MOCK_WRITABLE } }
    $o = [pscustomobject]@{ Owner = $owner; Aces = $aces }
    $o | Add-Member -MemberType ScriptMethod -Name GetOwner -Value { param($t) [pscustomobject]@{ Value = $this.Owner } }
    $o | Add-Member -MemberType ScriptMethod -Name GetAccessRules -Value {
        param($a, $b, $t)
        foreach ($ace in $this.Aces) {
            [pscustomobject]@{
                AccessControlType = 'Allow'
                PropagationFlags = [Security.AccessControl.PropagationFlags]::None
                IdentityReference = [pscustomobject]@{ Value = $ace.Sid }
                FileSystemRights = [Security.AccessControl.FileSystemRights]$ace.Rights
            }
        }
    }
    return $o
}
";

fn script(path: &Path, body: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

struct Machine {
    _dir: tempfile::TempDir,
    root: PathBuf,
    /// A user-writable directory that comes first on PATH.
    planted: PathBuf,
    /// The protected copy `Find-GowaySetup` should accept.
    protected: PathBuf,
}

fn machine(with_protected_copy: bool) -> Machine {
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    let planted = root.join("WindowsApps");
    for name in ["goway-setup", "goway-setup.exe", "goway-setup.cmd"] {
        script(
            &planted.join(name),
            &format!("echo PLANTED >> '{}'", root.join("marker").display()),
        );
    }
    let protected = root.join("ProgramData/goway/goway/bin/goway-setup.exe");
    if with_protected_copy {
        script(
            &protected,
            &format!(
                "echo REAL >> '{m}'; for a in \"$@\"; do printf '%s\\n' \"$a\" >> '{r}'; done; exit 7",
                m = root.join("marker").display(),
                r = root.join("args").display()
            ),
        );
    }
    Machine {
        _dir: dir,
        root,
        planted,
        protected,
    }
}

impl Machine {
    fn run(&self, step: &WinStep, mock: &[(&str, &str)]) -> Option<Output> {
        let pwsh = pwsh::pwsh()?;
        let source = format!("{MOCK_ACL}\n{}", step.admin_source());
        let path = format!(
            "{}:{}",
            self.planted.display(),
            std::env::var("PATH").unwrap_or_default()
        );
        let mut cmd = Command::new(pwsh);
        cmd.args(["-NoProfile", "-NonInteractive", "-Command", &source])
            .env("PATH", path)
            .env("ProgramFiles", self.root.join("ProgramFiles"))
            .env("ProgramData", self.root.join("ProgramData"));
        for (k, v) in mock {
            cmd.env(k, v);
        }
        Some(cmd.output().unwrap())
    }

    fn marker(&self) -> String {
        std::fs::read_to_string(self.root.join("marker")).unwrap_or_default()
    }
}

fn step() -> WinStep {
    WinStep::setup(
        &[
            "install",
            "--host",
            "--authorized-key",
            "ssh-ed25519 AAAA it\u{2019}s $(x) `n",
        ],
        "authorizes goway's key",
    )
}

fn refused(out: &Output) -> bool {
    out.status.code() == Some(EXIT_NO_TRUSTED_SETUP)
        && String::from_utf8_lossy(&out.stderr).contains("not found in a protected location")
}

#[test]
fn a_goway_setup_planted_in_the_path_never_runs() {
    let m = machine(false);
    let Some(out) = m.run(&step(), &[]) else {
        return;
    };
    assert!(refused(&out), "{out:?}");
    assert_eq!(m.marker(), "", "the planted program ran");
}

#[test]
fn a_protected_copy_runs_by_absolute_path_with_the_arguments_intact_and_its_exit_code() {
    let m = machine(true);
    let Some(out) = m.run(&step(), &[]) else {
        return;
    };
    assert_eq!(out.status.code(), Some(7), "{out:?}");
    assert_eq!(m.marker(), "REAL\n", "only the protected copy may run");
    let args = std::fs::read_to_string(m.root.join("args")).unwrap();
    assert_eq!(
        args,
        "install\n--host\n--authorized-key\nssh-ed25519 AAAA it\u{2019}s $(x) `n\n"
    );
}

#[test]
fn a_copy_that_a_normal_user_could_have_written_is_refused() {
    let m = machine(true);
    let file = "goway-setup.exe";
    let dir = "goway/goway/bin";
    for (what, mock) in [
        (
            "owned by the user",
            vec![("MOCK_MATCH", file), ("MOCK_OWNER", "S-1-5-21-1-2-3-1001")],
        ),
        (
            "file writable by Users",
            vec![("MOCK_MATCH", file), ("MOCK_WRITABLE", "Modify")],
        ),
        (
            "directory writable by Users",
            vec![("MOCK_MATCH", dir), ("MOCK_WRITABLE", "CreateFiles")],
        ),
        (
            "directory may be deleted from by Users",
            vec![
                ("MOCK_MATCH", dir),
                ("MOCK_WRITABLE", "DeleteSubdirectoriesAndFiles"),
            ],
        ),
        (
            "the profile directory is owned by the user",
            vec![
                ("MOCK_MATCH", "ProgramData/goway/goway"),
                ("MOCK_OWNER", "S-1-5-21-1-2-3-1001"),
            ],
        ),
    ] {
        let Some(out) = m.run(&step(), &mock) else {
            return;
        };
        assert!(refused(&out), "{what}: {out:?}");
        assert_eq!(m.marker(), "", "{what}: something ran");
    }
}

#[test]
fn a_linked_copy_is_refused() {
    let m = machine(true);
    let real = m.root.join("elsewhere.exe");
    std::fs::rename(&m.protected, &real).unwrap();
    std::os::unix::fs::symlink(&real, &m.protected).unwrap();
    let Some(out) = m.run(&step(), &[]) else {
        return;
    };
    assert!(refused(&out), "{out:?}");
    assert_eq!(m.marker(), "");
}
