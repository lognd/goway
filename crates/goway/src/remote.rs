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

/// The line [`ps_invocation`] writes to stderr when the script is missing,
/// so a caller that only sees an error message can tell it from a failure.
pub const NOT_INSTALLED_MARK: &str = "goway-script-not-installed";

/// What [`ps_probe`] prints (and exits 0) when the script is missing.
pub const PROBE_NEEDS_INSTALL: &str = "goway-needs-install";

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
        "{}; if (-not [IO.File]::Exists($p)) {{ [Console]::Error.WriteLine('{NOT_INSTALLED_MARK}'); exit {PS_NOT_INSTALLED} }}; & {}; exit $LASTEXITCODE",
        ps_locate(),
        words.join(" ")
    )
}

/// [`ps_invocation`] for a probe: a missing script is answered with
/// [`PROBE_NEEDS_INSTALL`] on stdout and exit 0, so address resolution (which
/// treats any failure as "wrong address") still succeeds and the caller
/// installs the script and probes again.
pub fn ps_probe(verb: &str, args: &[&str]) -> String {
    let mut words = vec!["$p".to_owned(), crate::transport::ps_quote(verb)];
    words.extend(args.iter().map(|a| crate::transport::ps_quote(a)));
    format!(
        "{}; if (-not [IO.File]::Exists($p)) {{ [Console]::Out.WriteLine('{PROBE_NEEDS_INSTALL}'); exit 0 }}; & {}; exit $LASTEXITCODE",
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
         foreach ($f in @([IO.Directory]::GetFiles($d, 'remote-*.ps1')) + @([IO.Directory]::GetFiles($d, 'remote-*.native.dll'))) {{ \
         if ($f -ne $p -and [IO.Path]::GetFileNameWithoutExtension($f).Replace('.native', '') -ne [IO.Path]::GetFileNameWithoutExtension($p) \
         -and [IO.File]::GetLastWriteTimeUtc($f) -lt [DateTime]::UtcNow.AddDays(-1)) {{ \
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

/// One call of a remote verb, before it is turned into the language of the
/// host that will run it (bash for Unix hosts, PowerShell for Windows).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Call {
    /// The verb (`manifest`, `run`, `probe`, ...).
    pub verb: String,
    /// Its arguments, exactly as the verb takes them.
    pub args: Vec<String>,
}

impl Call {
    /// A call of `verb` with `args`.
    pub fn new<S: AsRef<str>>(verb: &str, args: &[S]) -> Self {
        Self {
            verb: verb.to_owned(),
            args: args.iter().map(|a| a.as_ref().to_owned()).collect(),
        }
    }

    fn arg_refs(&self) -> Vec<&str> {
        self.args.iter().map(String::as_str).collect()
    }

    /// The command line for a Unix host.
    pub fn bash(&self) -> String {
        invocation(&self.verb, &self.arg_refs())
    }

    /// The PowerShell source for a Windows host.
    pub fn powershell(&self) -> String {
        ps_invocation(&self.verb, &self.arg_refs())
    }

    /// The PowerShell source for a probe of a Windows host (see [`ps_probe`]).
    pub fn powershell_probe(&self) -> String {
        ps_probe(&self.verb, &self.arg_refs())
    }
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

    // frob:tests crates/goway/src/remote.rs::Call
    #[test]
    fn a_call_renders_for_both_languages() {
        let c = Call::new("manifest", &["root", "it's"]);
        assert!(c.bash().starts_with("bash -c "));
        let ps = c.powershell();
        assert!(ps.contains("& $p 'manifest' 'root' 'it''s'"), "{ps}");
        assert!(ps.contains(NOT_INSTALLED_MARK));
        let probe = Call::new("probe", &["root"]).powershell_probe();
        assert!(probe.contains(PROBE_NEEDS_INSTALL) && probe.contains("exit 0"));
    }
}
