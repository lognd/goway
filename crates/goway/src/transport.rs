//! One way to start a command on any kind of host.
//!
//! A host is reached in one of three ways today (a fourth, "in place on
//! this machine", is [`crate::local`]): a Unix shell over ssh (Linux and
//! WSL helpers, `remote.sh`), PowerShell over ssh (a Windows machine's
//! OpenSSH server, `remote.ps1`), or PowerShell through WSL interop (the
//! Windows side of this machine, no ssh). [`Kind`] names them and
//! [`command`] builds the process that runs a script there, so callers
//! hold one abstraction instead of branching on the operating system.
//!
//! PowerShell receives every script as `-EncodedCommand` (base64 of
//! UTF-16LE): the text arrives exactly, whatever quotes, dollar signs or
//! spaces it holds, and no layer re-splits it. Arguments become PowerShell
//! source with [`ps_call`]: each is single-quoted with embedded quotes
//! doubled, and the call operator `&` runs the program.

use std::process::Command;

use base64::Engine as _;

use crate::config::{HostConfig, Os, Transport};
use crate::error::{Error, Result};
use crate::remote::Call;
use crate::ssh::{self, KeyPolicy, Settings, Target};

/// The PowerShell options goway always passes, ending with the one that
/// takes the encoded script as the next argument.
pub const POWERSHELL_FLAGS: [&str; 5] = [
    "-NoProfile",
    "-NonInteractive",
    "-ExecutionPolicy",
    "Bypass",
    "-EncodedCommand",
];

/// How a host is reached and what it speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Linux (including WSL) over ssh: scripts are `bash` text.
    Unix,
    /// A Windows machine's OpenSSH server: scripts are PowerShell.
    WindowsSsh,
    /// The Windows side of this machine through `powershell.exe`: no ssh.
    WindowsInterop,
}

impl Kind {
    /// The kind of `host` from its `os` and `transport` settings.
    pub fn of(host: &HostConfig) -> Self {
        match (host.os, host.transport) {
            (Os::Linux, _) => Self::Unix,
            (Os::Windows, Transport::Ssh) => Self::WindowsSsh,
            (Os::Windows, Transport::Interop) => Self::WindowsInterop,
        }
    }

    /// Whether goway reaches this kind with ssh (pinned key, no password).
    pub fn uses_ssh(self) -> bool {
        !matches!(self, Self::WindowsInterop)
    }

    /// The operating system's name as the probe reports it.
    pub fn os(self) -> Os {
        match self {
            Self::Unix => Os::Linux,
            Self::WindowsSsh | Self::WindowsInterop => Os::Windows,
        }
    }
}

/// A script in the language the host speaks.
#[derive(Debug, Clone, Copy)]
pub enum Script<'a> {
    /// A POSIX shell command line (what ssh sends to a Unix host).
    Sh(&'a str),
    /// PowerShell source.
    Ps(&'a str),
}

/// Single-quote `arg` for PowerShell: every quote character PowerShell recognizes (the ASCII
/// one and the typographic U+2018 to U+201B) is doubled, so nothing in it is ever interpreted
/// (no `$`, backtick or splitting). The one quoter, shared with goway-setup.
pub fn ps_quote(arg: &str) -> String {
    goway_journal::ps_quote(arg)
}

/// PowerShell source that runs `words` (program first) with each word
/// passed exactly as given: `& 'prog' 'arg1' 'arg2'`.
pub fn ps_call<S: AsRef<str>>(words: &[S]) -> String {
    let quoted: Vec<String> = words.iter().map(|w| ps_quote(w.as_ref())).collect();
    format!("& {}", quoted.join(" "))
}

/// The base64 of `source` as UTF-16LE, what `-EncodedCommand` takes.
pub fn encoded_command(source: &str) -> String {
    let bytes: Vec<u8> = source.encode_utf16().flat_map(u16::to_le_bytes).collect();
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// The command line a Windows OpenSSH server is given to run `source`
/// (its default shell may be cmd.exe, so the script is encoded).
pub fn windows_ssh_line(source: &str) -> String {
    format!(
        "powershell {} {}",
        POWERSHELL_FLAGS.join(" "),
        encoded_command(source)
    )
}

/// The process that runs `script` on a host of `kind`: ssh to `target`
/// with the pinned-key rules of [`ssh::command`] for the ssh kinds,
/// `powershell.exe` for interop. A script in the wrong language for the
/// kind is a goway bug and an error.
pub fn command(
    kind: Kind,
    target: &Target,
    settings: &Settings,
    policy: KeyPolicy,
    script: Script<'_>,
) -> Result<Command> {
    match (kind, script) {
        (Kind::Unix, Script::Sh(line)) => Ok(ssh::command(target, settings, policy, line)),
        (Kind::WindowsSsh, Script::Ps(source)) => Ok(ssh::command(
            target,
            settings,
            policy,
            &windows_ssh_line(source),
        )),
        (Kind::WindowsInterop, Script::Ps(source)) => crate::interop::command(source),
        (kind, script) => Err(Error::Usage(format!(
            "a {script:?} script cannot run on a {kind:?} host"
        ))),
    }
}

/// The remote command line an ssh host is given to run `call`: bash text
/// for a Unix host, an encoded PowerShell line for a Windows one. Interop
/// has no command line (see [`call_command`]).
pub fn ssh_line(kind: Kind, call: &Call) -> String {
    match kind {
        Kind::Unix => call.bash(),
        Kind::WindowsSsh | Kind::WindowsInterop => windows_ssh_line(&call.powershell()),
    }
}

/// [`ssh_line`] for a probe: on a Windows host a missing script answers
/// [`crate::remote::PROBE_NEEDS_INSTALL`] instead of failing.
pub fn probe_line(kind: Kind, call: &Call) -> String {
    match kind {
        Kind::Unix => call.bash(),
        Kind::WindowsSsh | Kind::WindowsInterop => windows_ssh_line(&call.powershell_probe()),
    }
}

/// The process that runs `call` on a host of `kind`, in the host's own
/// language.
pub fn call_command(
    kind: Kind,
    target: &Target,
    settings: &Settings,
    policy: KeyPolicy,
    call: &Call,
) -> Result<Command> {
    let script = match kind {
        Kind::Unix => return Ok(ssh::command(target, settings, policy, &call.bash())),
        Kind::WindowsSsh | Kind::WindowsInterop => call.powershell(),
    };
    command(kind, target, settings, policy, Script::Ps(&script))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host(os: Os, transport: Transport) -> HostConfig {
        HostConfig {
            name: "helios".to_owned(),
            os,
            transport,
            ..HostConfig::default()
        }
    }

    // frob:tests crates/goway/src/transport.rs::Kind
    #[test]
    fn kinds_follow_os_and_transport() {
        assert_eq!(Kind::of(&host(Os::Linux, Transport::Ssh)), Kind::Unix);
        assert_eq!(
            Kind::of(&host(Os::Windows, Transport::Ssh)),
            Kind::WindowsSsh
        );
        assert_eq!(
            Kind::of(&host(Os::Windows, Transport::Interop)),
            Kind::WindowsInterop
        );
        assert!(Kind::WindowsSsh.uses_ssh() && !Kind::WindowsInterop.uses_ssh());
    }

    // frob:tests crates/goway/src/transport.rs::ps_quote
    // frob:tests crates/goway/src/transport.rs::ps_call
    #[test]
    fn arguments_are_quoted_for_powershell_exactly() {
        assert_eq!(ps_quote("it's"), "'it''s'");
        assert_eq!(
            ps_call(&["cargo", "test", "$env:X `n", "a b", "\"q\"", ""]),
            "& 'cargo' 'test' '$env:X `n' 'a b' '\"q\"' ''"
        );
    }

    // frob:tests crates/goway/src/transport.rs::encoded_command
    #[test]
    fn encoded_commands_are_utf16le_base64() {
        // "hi" is 68 00 69 00.
        assert_eq!(encoded_command("hi"), "aABpAA==");
        assert!(windows_ssh_line("hi").ends_with(" -EncodedCommand aABpAA=="));
    }

    // frob:tests crates/goway/src/transport.rs::command
    #[test]
    fn a_script_in_the_wrong_language_is_refused() {
        let target = Target {
            name: "helios".to_owned(),
            address: "192.0.2.1".to_owned(),
            port: 22,
            user: None,
            identity: None,
        };
        let settings = Settings {
            known_hosts: std::path::PathBuf::from("kh"),
            control_dir: None,
            connect_timeout_secs: 5,
        };
        assert!(
            command(
                Kind::Unix,
                &target,
                &settings,
                KeyPolicy::Strict,
                Script::Ps("x")
            )
            .is_err()
        );
        assert!(
            command(
                Kind::WindowsSsh,
                &target,
                &settings,
                KeyPolicy::Strict,
                Script::Sh("x")
            )
            .is_err()
        );
    }

    // frob:tests crates/goway/src/transport.rs::ssh_line
    // frob:tests crates/goway/src/transport.rs::probe_line
    #[test]
    fn lines_follow_the_kind() {
        let call = Call::new("ping", &[] as &[&str]);
        assert!(ssh_line(Kind::Unix, &call).starts_with("bash -c "));
        assert!(ssh_line(Kind::WindowsSsh, &call).starts_with("powershell -NoProfile"));
        assert!(probe_line(Kind::Unix, &call).starts_with("bash -c "));
        assert_ne!(
            ssh_line(Kind::WindowsSsh, &call),
            probe_line(Kind::WindowsSsh, &call)
        );
    }
}
