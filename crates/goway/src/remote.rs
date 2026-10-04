//! The embedded remote script and how to invoke its verbs over ssh.

use crate::ssh::shell_join;

/// The remote side of goway, sent inline with each call.
pub const SCRIPT: &str = include_str!("remote.sh");

/// The remote side of goway for Windows hosts (PowerShell): the same verbs
/// and protocol as [`SCRIPT`]. It is too large for the command line of a
/// Windows OpenSSH server, so it is installed once per version under the
/// user's local application data and each call runs the installed copy.
pub const SCRIPT_PS: &str = include_str!("remote.ps1");

/// The exit code of [`ps_invocation`] when this version of the script is not
/// installed on the host yet: send [`ps_install`] the script, then retry.
pub const PS_NOT_INSTALLED: i32 = 126;

/// A short id of this version of [`SCRIPT_PS`] (names the installed file).
pub fn ps_script_id() -> String {
    use sha2::Digest as _;
    let hash = sha2::Sha256::digest(SCRIPT_PS.as_bytes());
    hash.iter().take(6).fold(String::new(), |mut s, b| {
        use std::fmt::Write as _;
        let _ = write!(s, "{b:02x}");
        s
    })
}

/// PowerShell source that sets `$p` to the installed script's path.
fn ps_locate() -> String {
    format!(
        "$p = [IO.Path]::Combine([Environment]::GetFolderPath('LocalApplicationData'), 'goway', 'remote-{}.ps1')",
        ps_script_id()
    )
}

/// PowerShell source running `verb` with `args` on a Windows host: every
/// argument is single-quoted (embedded quotes doubled), so it arrives
/// exactly. Exits with [`PS_NOT_INSTALLED`] when the script is missing, and
/// otherwise with the script's own exit code.
pub fn ps_invocation(verb: &str, args: &[&str]) -> String {
    let mut words = vec!["$p".to_owned(), crate::transport::ps_quote(verb)];
    words.extend(args.iter().map(|a| crate::transport::ps_quote(a)));
    format!(
        "{}; if (-not [IO.File]::Exists($p)) {{ exit {PS_NOT_INSTALLED} }}; & {}; exit $LASTEXITCODE",
        ps_locate(),
        words.join(" ")
    )
}

/// PowerShell source that installs [`SCRIPT_PS`] from its standard input
/// (the script bytes), atomically, and removes copies of other versions that
/// have not been used for a day.
pub fn ps_install() -> String {
    format!(
        "{}; $d = [IO.Path]::GetDirectoryName($p); [void][IO.Directory]::CreateDirectory($d); \
         $ms = New-Object IO.MemoryStream; [Console]::OpenStandardInput().CopyTo($ms); \
         $t = \"$p.$PID.tmp\"; [IO.File]::WriteAllBytes($t, $ms.ToArray()); \
         if ([IO.File]::Exists($p)) {{ [IO.File]::Delete($p) }}; [IO.File]::Move($t, $p); \
         foreach ($f in [IO.Directory]::GetFiles($d, 'remote-*.ps1')) {{ \
         if ($f -ne $p -and [IO.File]::GetLastWriteTimeUtc($f) -lt [DateTime]::UtcNow.AddDays(-1)) {{ \
         try {{ [IO.File]::Delete($f) }} catch {{ }} }} }}; exit 0",
        ps_locate()
    )
}

/// The remote command line running `verb` with `args`.
pub fn invocation(verb: &str, args: &[&str]) -> String {
    let mut words = vec!["bash", "-c", SCRIPT, "goway", verb];
    words.extend_from_slice(args);
    shell_join(&words)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invocation_runs_locally_through_a_shell() {
        let out = std::process::Command::new("sh")
            .args(["-c", &invocation("ping", &[])])
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout), "goway-remote ok\n");
        let out = std::process::Command::new("sh")
            .args(["-c", &invocation("nope", &["it's"])])
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(125));
        assert!(String::from_utf8_lossy(&out.stderr).contains("unknown verb"));
    }
}
