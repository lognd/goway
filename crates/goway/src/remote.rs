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

/// The line a Unix helper's stdout starts with once the remote script runs:
/// everything before it is startup-file noise (a `.bashrc` that echoes, a
/// login shell's greeting) and is not goway's protocol. Starts with a
/// control character no startup file prints by accident.
pub const FRAME_MARK: &[u8] = b"\x01goway-frame\n";

/// Split `stdout` of a Unix helper call at [`FRAME_MARK`]: the noise before
/// it and the protocol output after it. Output without the mark (a Windows
/// helper, which has no startup files in the way) is all payload.
pub fn split_frame(stdout: Vec<u8>) -> (Vec<u8>, Vec<u8>) {
    let Some(at) = stdout
        .windows(FRAME_MARK.len())
        .position(|w| w == FRAME_MARK)
    else {
        return (Vec::new(), stdout);
    };
    let noise = stdout[..at].to_vec();
    let mut payload = stdout;
    payload.drain(..at + FRAME_MARK.len());
    (noise, payload)
}

/// Most startup-file noise [`Framed`] holds while looking for the mark; a
/// stream with more than this before any mark is passed through unchanged.
const MAX_NOISE: usize = 1 << 20;

/// A reader over the stdout of a streamed Unix helper call that drops the
/// startup-file noise before [`FRAME_MARK`] and then passes the rest through.
#[derive(Debug)]
pub struct Framed<R> {
    inner: R,
    held: Vec<u8>,
    seeking: bool,
}

impl<R: std::io::Read> Framed<R> {
    /// Frame `inner`.
    pub fn new(inner: R) -> Self {
        Self {
            inner,
            held: Vec::new(),
            seeking: true,
        }
    }

    /// Read until the mark is found (dropping what came before it), or give up.
    fn seek(&mut self) -> std::io::Result<()> {
        let mut chunk = [0u8; 8192];
        while self.seeking {
            let n = self.inner.read(&mut chunk)?;
            self.held.extend_from_slice(&chunk[..n]);
            let found = self
                .held
                .windows(FRAME_MARK.len())
                .position(|w| w == FRAME_MARK);
            if let Some(at) = found {
                if at > 0 {
                    tracing::warn!(
                        bytes = at,
                        "the helper's shell startup files print text; goway ignored it"
                    );
                }
                self.held.drain(..at + FRAME_MARK.len());
                self.seeking = false;
            } else if n == 0 || self.held.len() > MAX_NOISE {
                tracing::debug!("no frame mark in the helper's output; passing it through");
                self.seeking = false;
            }
        }
        Ok(())
    }
}

impl<R: std::io::Read> std::io::Read for Framed<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.seeking {
            self.seek()?;
        }
        if self.held.is_empty() {
            return self.inner.read(buf);
        }
        let n = buf.len().min(self.held.len());
        buf[..n].copy_from_slice(&self.held[..n]);
        self.held.drain(..n);
        Ok(n)
    }
}

/// `script` with every whole-line comment blanked (not removed, so error line numbers still
/// match the source): the comments are a third of the script and the line limit is tight.
/// The script has no heredocs or multi-line strings, so no content line starts with `#`.
fn without_comments(script: &str) -> String {
    let mut out = String::with_capacity(script.len());
    for line in script.lines() {
        if !line.trim_start().starts_with('#') {
            out.push_str(line);
        }
        out.push('\n');
    }
    out
}

/// The path of `remote.sh` in this source tree: tests run it as `bash <path> verb...`, since the
/// script is too large to pass as one `bash -c` argument (Linux caps one argument at 128 KiB).
pub const SCRIPT_SH_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/src/remote.sh");

/// The most the remote command line may be: a single argument is capped at
/// 128 KiB by Linux, and the encoded script is the bulk of the line.
pub const MAX_LINE: usize = 126_000;

/// The remote command line running `verb` with `args`, safe for any login
/// shell. The login shell only ever sees `bash -c '<fixed text>' goway`:
/// no quote, backslash, `!` or newline of the script or its arguments
/// reaches fish, csh or any other shell's parser. The script, with the
/// verb and its arguments set as positional parameters, travels as base64
/// and bash decodes and evaluates it; the first thing it prints is
/// [`FRAME_MARK`].
pub fn invocation(verb: &str, args: &[&str]) -> String {
    let payload = format!(
        "printf '\\001goway-frame\\n'\nset -- {}\n{}",
        shell_join(&verb_words(verb, args)),
        without_comments(SCRIPT)
    );
    wrap_payload(&payload)
}

/// `verb` followed by `args`, as one word list.
fn verb_words<'a>(verb: &'a str, args: &[&'a str]) -> Vec<&'a str> {
    let mut words = vec![verb];
    words.extend_from_slice(args);
    words
}

/// The login-shell-safe line that makes bash evaluate `payload`: the shell
/// only ever sees `bash -c '<fixed text>' goway` around base64.
fn wrap_payload(payload: &str) -> String {
    use base64::Engine as _;
    let b64 = base64::engine::general_purpose::STANDARD.encode(payload);
    format!("bash -c 'eval \"$(printf %s {b64} | base64 -d)\"' goway")
}

/// The full sha256 of [`SCRIPT`], lowercase hex: what a helper's installed
/// copy must hash to before it is run.
pub fn script_hash() -> String {
    use sha2::Digest as _;
    sha2::Sha256::digest(SCRIPT.as_bytes())
        .iter()
        .fold(String::new(), |mut s, b| {
            use std::fmt::Write as _;
            let _ = write!(s, "{b:02x}");
            s
        })
}

/// A short id of this version of [`SCRIPT`] (names the installed file).
pub fn script_id() -> String {
    script_hash().chars().take(12).collect()
}

/// Shell source that sets `d` and `f` to the installed script's directory and path.
/// The directory is `$XDG_CACHE_HOME/goway/bin` (else `~/.cache/goway/bin`).
fn sh_locate() -> String {
    format!(
        "d=\"${{XDG_CACHE_HOME:-$HOME/.cache}}/goway/bin\"; f=\"$d/remote-{}.sh\"",
        script_id()
    )
}

/// Shell source that sets `ok=1` when the file named by `var` hashes to
/// [`script_hash`]. Uses whichever of sha256sum, shasum or openssl exists;
/// with none of them the file is never trusted.
fn sh_verify(var: &str) -> String {
    format!(
        "h=$(sha256sum \"{var}\" 2>/dev/null || shasum -a 256 \"{var}\" 2>/dev/null || openssl dgst -sha256 \"{var}\" 2>/dev/null); ok=0; case \"$h\" in *{}*) ok=1;; esac",
        script_hash()
    )
}

/// The command line running `verb` with `args` from the copy of the script
/// installed on the helper, a few hundred bytes whatever the script's size.
/// A missing or modified copy (it must hash to [`script_hash`]) is answered
/// on stderr with [`NOT_INSTALLED_MARK`] and exit code [`PS_NOT_INSTALLED`]:
/// send [`unix_install`] the script, then retry.
pub fn installed_invocation(verb: &str, args: &[&str]) -> String {
    let payload = format!(
        "set -- {}\n{}\n{}\nif [ \"$ok\" = 1 ]; then printf '\\001goway-frame\\n'; exec bash \"$f\" \"$@\"; fi\necho {NOT_INSTALLED_MARK} >&2; exit {PS_NOT_INSTALLED}\n",
        shell_join(&verb_words(verb, args)),
        sh_locate(),
        sh_verify("$f"),
    );
    wrap_payload(&payload)
}

/// [`installed_invocation`] for a probe: a missing script is answered with
/// [`PROBE_NEEDS_INSTALL`] on stdout and exit 0, so address resolution
/// (which treats any failure as "wrong address") still succeeds and the
/// caller installs the script and probes again.
pub fn installed_probe(verb: &str, args: &[&str]) -> String {
    let payload = format!(
        "set -- {}\n{}\n{}\nif [ \"$ok\" = 1 ]; then printf '\\001goway-frame\\n'; exec bash \"$f\" \"$@\"; fi\necho {PROBE_NEEDS_INSTALL}; exit 0\n",
        shell_join(&verb_words(verb, args)),
        sh_locate(),
        sh_verify("$f"),
    );
    wrap_payload(&payload)
}

/// The command line that installs [`SCRIPT`] on a Unix helper from its
/// standard input (the script bytes): written beside the target, verified
/// against [`script_hash`], then moved into place atomically, and copies of
/// other versions older than a day are removed. Fails (non-zero) when the
/// home is read-only or no sha256 tool exists.
pub fn unix_install() -> String {
    let payload = format!(
        "{}\numask 077; mkdir -p \"$d\" || exit 1; t=\"$f.$$.tmp\"\n\
         cat > \"$t\" || {{ rm -f \"$t\"; exit 1; }}\n{}\n\
         if [ \"$ok\" != 1 ]; then rm -f \"$t\"; echo goway-script-hash-mismatch >&2; exit 1; fi\n\
         chmod 700 \"$t\" && mv -f \"$t\" \"$f\" || {{ rm -f \"$t\"; exit 1; }}\n\
         find \"$d\" -name 'remote-*.sh' ! -name \"remote-{}.sh\" -mtime +0 -exec rm -f {{}} + 2>/dev/null\nexit 0\n",
        sh_locate(),
        sh_verify("$t"),
        script_id(),
    );
    wrap_payload(&payload)
}

/// Hosts whose helper could not keep an installed copy (read-only home, no
/// hash tool): their calls carry the whole script, as before.
static INLINE_ONLY: std::sync::Mutex<std::collections::BTreeSet<String>> =
    std::sync::Mutex::new(std::collections::BTreeSet::new());

/// Remember that `host` cannot keep an installed script, so its calls send
/// the whole script inline from now on (this process only).
pub fn mark_inline_only(host: &str) {
    tracing::warn!(
        host,
        "the helper cannot keep the remote script; sending it with every call"
    );
    if let Ok(mut set) = INLINE_ONLY.lock() {
        set.insert(host.to_owned());
    }
}

/// Whether `host` was marked by [`mark_inline_only`].
pub fn is_inline_only(host: &str) -> bool {
    INLINE_ONLY.lock().is_ok_and(|set| set.contains(host))
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

    /// The command line for a Unix host that carries the whole script (this
    /// machine, or a helper that cannot keep an installed copy).
    pub fn bash(&self) -> String {
        invocation(&self.verb, &self.arg_refs())
    }

    /// The short command line for a Unix host that runs its installed copy
    /// of the script (see [`installed_invocation`]).
    pub fn bash_installed(&self) -> String {
        installed_invocation(&self.verb, &self.arg_refs())
    }

    /// [`Call::bash_installed`] for a probe (see [`installed_probe`]).
    pub fn bash_probe(&self) -> String {
        installed_probe(&self.verb, &self.arg_refs())
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
        let (noise, payload) = split_frame(out.stdout);
        assert!(noise.is_empty());
        assert_eq!(String::from_utf8_lossy(&payload), "goway-remote ok\n");
        let out = std::process::Command::new("sh")
            .args(["-c", &invocation("nope", &["it's"])])
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(125));
        assert!(String::from_utf8_lossy(&out.stderr).contains("unknown verb"));
    }

    // frob:tests crates/goway/src/remote.rs::split_frame
    #[test]
    fn the_frame_mark_splits_noise_from_the_payload() {
        let mut wire = b"Welcome back!\n".to_vec();
        wire.extend_from_slice(FRAME_MARK);
        wire.extend_from_slice(b"payload\n");
        let (noise, payload) = split_frame(wire);
        assert_eq!(noise, b"Welcome back!\n");
        assert_eq!(payload, b"payload\n");
        // No mark (a Windows helper): everything is payload.
        let (noise, payload) = split_frame(b"plain\n".to_vec());
        assert!(noise.is_empty());
        assert_eq!(payload, b"plain\n");
    }

    // frob:tests crates/goway/src/remote.rs::Framed
    #[test]
    fn a_framed_reader_drops_noise_even_when_the_mark_is_split_across_reads() {
        use std::io::Read as _;
        // A reader that hands out one byte at a time splits the mark everywhere.
        struct Drip(std::io::Cursor<Vec<u8>>);
        impl std::io::Read for Drip {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                let n = 1.min(buf.len());
                self.0.read(&mut buf[..n])
            }
        }
        let mut wire = b"motd line\nanother\n".to_vec();
        wire.extend_from_slice(FRAME_MARK);
        wire.extend_from_slice(b"job output\n");
        let mut got = String::new();
        Framed::new(Drip(std::io::Cursor::new(wire)))
            .read_to_string(&mut got)
            .unwrap();
        assert_eq!(got, "job output\n");
        // Without a mark the stream is passed through unchanged.
        let mut got = String::new();
        Framed::new(&b"no mark here"[..])
            .read_to_string(&mut got)
            .unwrap();
        assert_eq!(got, "no mark here");
    }

    // frob:tests crates/goway/src/remote.rs::invocation
    #[test]
    fn the_command_line_is_plain_text_any_login_shell_parses_and_fits() {
        // A script and arguments full of quotes, backslashes, bangs and
        // newlines reach the login shell only as base64.
        let line = invocation("run", &["it's", "a\\b", "!x", "two\nlines"]);
        assert!(line.starts_with("bash -c 'eval \"$(printf %s "), "{line}");
        assert!(line.ends_with(" | base64 -d)\"' goway"), "{line}");
        assert!(!line.contains('\n') && !line.contains('\\') && !line.contains('!'));
        assert_eq!(line.matches('\'').count(), 2, "one quoted word only");
        assert!(
            line.len() < MAX_LINE,
            "the encoded script is {} bytes; the limit is {MAX_LINE}",
            line.len()
        );
    }

    // frob:tests crates/goway/src/remote.rs::invocation
    #[test]
    fn the_longest_realistic_command_line_stays_under_the_limit() {
        // The arguments travel as base64 too: 4 KiB of them is well past any real command.
        let long = "x".repeat(4 * 1024);
        let line = invocation("run", &[long.as_str(), "--", "cargo", "nextest", "run"]);
        assert!(
            line.len() < MAX_LINE,
            "the encoded command line is {} bytes; the limit is {MAX_LINE} (one argument may not exceed 131072)",
            line.len()
        );
    }

    #[cfg(unix)]
    /// Run `line` through `sh -c` with a private cache dir and `input` on stdin.
    fn run_line(cache: &std::path::Path, line: &str, input: &[u8]) -> std::process::Output {
        use std::io::Write as _;
        let mut child = std::process::Command::new("sh")
            .args(["-c", line])
            .env("XDG_CACHE_HOME", cache)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let _ = child.stdin.take().unwrap().write_all(input);
        child.wait_with_output().unwrap()
    }

    #[cfg(unix)]
    fn installed_file(cache: &std::path::Path) -> std::path::PathBuf {
        cache
            .join("goway/bin")
            .join(format!("remote-{}.sh", script_id()))
    }

    // frob:ticket 01M4557B4FQ51Y1T49Y0B12Y4D
    // frob:tests crates/goway/src/remote.rs::installed_invocation
    // frob:tests crates/goway/src/remote.rs::unix_install
    #[cfg(unix)]
    #[test]
    fn a_helper_without_the_script_gets_it_once_and_then_runs_it_from_disk() {
        let cache = tempfile::tempdir().unwrap();
        let line = installed_invocation("ping", &[]);
        let out = run_line(cache.path(), &line, b"");
        assert_eq!(out.status.code(), Some(PS_NOT_INSTALLED));
        assert!(String::from_utf8_lossy(&out.stderr).contains(NOT_INSTALLED_MARK));
        assert!(out.stdout.is_empty());

        let out = run_line(cache.path(), &unix_install(), SCRIPT.as_bytes());
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(
            std::fs::read_to_string(installed_file(cache.path())).unwrap(),
            SCRIPT
        );

        let out = run_line(cache.path(), &line, b"");
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let (noise, payload) = split_frame(out.stdout);
        assert!(noise.is_empty());
        assert_eq!(String::from_utf8_lossy(&payload), "goway-remote ok\n");

        // Arguments arrive exactly, and an unknown verb is the script's own error.
        let out = run_line(cache.path(), &installed_invocation("nope", &["it's"]), b"");
        assert_eq!(out.status.code(), Some(125));
        assert!(String::from_utf8_lossy(&out.stderr).contains("unknown verb"));
    }

    // frob:ticket 01M4557B4FQ51Y1T49Y0B12Y4D
    // frob:tests crates/goway/src/remote.rs::installed_probe
    #[cfg(unix)]
    #[test]
    fn a_probe_of_a_helper_without_the_script_asks_for_an_install() {
        let cache = tempfile::tempdir().unwrap();
        let out = run_line(cache.path(), &installed_probe("ping", &[]), b"");
        assert!(out.status.success());
        assert_eq!(
            String::from_utf8_lossy(&out.stdout).trim(),
            PROBE_NEEDS_INSTALL
        );
    }

    // frob:ticket 01M4557B4FQ51Y1T49Y0B12Y4D
    // frob:tests crates/goway/src/remote.rs::installed_invocation
    #[cfg(unix)]
    #[test]
    fn a_modified_installed_script_is_refused_and_reinstalled() {
        let cache = tempfile::tempdir().unwrap();
        let line = installed_invocation("ping", &[]);
        assert!(
            run_line(cache.path(), &unix_install(), SCRIPT.as_bytes())
                .status
                .success()
        );
        let file = installed_file(cache.path());
        std::fs::write(&file, format!("{SCRIPT}\necho tampered\n")).unwrap();

        let out = run_line(cache.path(), &line, b"");
        assert_eq!(out.status.code(), Some(PS_NOT_INSTALLED));
        assert!(!String::from_utf8_lossy(&out.stdout).contains("tampered"));

        assert!(
            run_line(cache.path(), &unix_install(), SCRIPT.as_bytes())
                .status
                .success()
        );
        let out = run_line(cache.path(), &line, b"");
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    // frob:ticket 01M4557B4FQ51Y1T49Y0B12Y4D
    // frob:tests crates/goway/src/remote.rs::unix_install
    #[cfg(unix)]
    #[test]
    fn an_install_of_bytes_that_do_not_hash_right_is_refused() {
        let cache = tempfile::tempdir().unwrap();
        let out = run_line(cache.path(), &unix_install(), b"not the script");
        assert!(!out.status.success());
        assert!(!installed_file(cache.path()).exists());
    }

    // frob:ticket 01M4557B4FQ51Y1T49Y0B12Y4D
    // frob:tests crates/goway/src/remote.rs::unix_install
    #[cfg(unix)]
    #[test]
    fn an_install_into_a_read_only_home_fails_so_the_caller_falls_back() {
        use std::os::unix::fs::PermissionsExt as _;
        let cache = tempfile::tempdir().unwrap();
        std::fs::set_permissions(cache.path(), std::fs::Permissions::from_mode(0o500)).unwrap();
        let out = run_line(cache.path(), &unix_install(), SCRIPT.as_bytes());
        std::fs::set_permissions(cache.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        // Running as root ignores the mode; only assert when the dir was really read-only.
        if !installed_file(cache.path()).exists() {
            assert!(!out.status.success());
        }
    }

    // frob:ticket 01M4557B4FQ51Y1T49Y0B12Y4D
    // frob:tests crates/goway/src/remote.rs::installed_invocation
    #[test]
    fn the_installed_command_line_stays_short_whatever_the_script_size() {
        let short = installed_invocation("probe", &["a", "b"]);
        assert!(short.len() < 4096, "{} bytes", short.len());
        assert!(
            short.len() * 20 < SCRIPT.len(),
            "must not grow with remote.sh"
        );
        assert!(short.starts_with("bash -c 'eval \"$(printf %s "), "{short}");
        assert!(!short.contains('\n') && !short.contains('\\') && !short.contains('!'));
        let long = "x".repeat(2 * 1024);
        let line = installed_invocation("run", &[long.as_str(), "--", "cargo", "test"]);
        assert!(line.len() < 8192, "{} bytes", line.len());
        assert!(unix_install().len() < 4096);
        assert!(installed_probe("probe", &[]).len() < 4096);
    }

    // frob:tests crates/goway/src/remote.rs::mark_inline_only
    #[test]
    fn a_host_marked_inline_only_is_remembered() {
        assert!(!is_inline_only("never-marked-host"));
        mark_inline_only("marked-host-for-test");
        assert!(is_inline_only("marked-host-for-test"));
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
