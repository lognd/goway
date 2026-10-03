//! Building ssh invocations. goway spawns the system `ssh` so the user's
//! agent, config and the Windows OpenSSH client all just work.
//!
//! Identity is pinned per host: every call uses `HostKeyAlias=goway-<name>`
//! with goway's own `known_hosts`, so the key is bound to the name and the
//! address is free to change. A wrong address fails the host key check.

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::Command;

use crate::config::key_alias;
use crate::paths::Paths;

/// One concrete ssh destination: a host identity at a resolved address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    /// The host name (identity).
    pub name: String,
    /// The address to connect to now.
    pub address: String,
    /// The ssh port.
    pub port: u16,
    /// The remote user, if not left to ssh config.
    pub user: Option<String>,
}

/// How to treat a host key not yet in goway's `known_hosts`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyPolicy {
    /// Only connect to the pinned key (every normal call).
    Strict,
    /// Pin the key on first contact (`goway host add`).
    AcceptNew,
}

/// Settings shared by every ssh call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    /// goway's `known_hosts` file.
    pub known_hosts: PathBuf,
    /// `ControlMaster` socket directory; `None` disables multiplexing.
    pub control_dir: Option<PathBuf>,
    /// Seconds to wait for the TCP connection.
    pub connect_timeout_secs: u32,
}

impl Settings {
    /// Settings from goway's paths; multiplexing only on Unix clients
    /// (Windows OpenSSH has no `ControlMaster`).
    pub fn from_paths(paths: &Paths) -> Self {
        Self {
            known_hosts: paths.known_hosts(),
            control_dir: cfg!(unix).then(|| paths.control_dir()),
            connect_timeout_secs: 5,
        }
    }
}

/// Quote an ssh option value; ssh splits option values on whitespace.
fn option_value(path: &std::path::Path) -> String {
    let s = path.to_string_lossy();
    if s.contains(char::is_whitespace) {
        format!("\"{s}\"")
    } else {
        s.into_owned()
    }
}

/// The ssh arguments up to and including the destination.
pub fn args(target: &Target, settings: &Settings, policy: KeyPolicy) -> Vec<OsString> {
    let strict = match policy {
        KeyPolicy::Strict => "yes",
        KeyPolicy::AcceptNew => "accept-new",
    };
    let mut opts = vec![
        "BatchMode=yes".to_owned(),
        format!("HostKeyAlias={}", key_alias(&target.name)),
        format!("StrictHostKeyChecking={strict}"),
        format!("UserKnownHostsFile={}", option_value(&settings.known_hosts)),
        "GlobalKnownHostsFile=none".to_owned(),
        "HashKnownHosts=no".to_owned(),
        format!("ConnectTimeout={}", settings.connect_timeout_secs),
        "ServerAliveInterval=15".to_owned(),
        "ServerAliveCountMax=4".to_owned(),
        "LogLevel=ERROR".to_owned(),
    ];
    if let Some(dir) = &settings.control_dir {
        opts.push("ControlMaster=auto".to_owned());
        opts.push(format!("ControlPath={}", option_value(&dir.join("%C"))));
        opts.push("ControlPersist=60s".to_owned());
    }
    let mut out: Vec<OsString> = Vec::new();
    for o in opts {
        out.push("-o".into());
        out.push(o.into());
    }
    out.push("-p".into());
    out.push(target.port.to_string().into());
    if let Some(user) = &target.user {
        out.push("-l".into());
        out.push(user.into());
    }
    out.push("--".into());
    out.push(target.address.clone().into());
    out
}

/// An ssh command running `remote` (a shell command line) on `target`.
pub fn command(target: &Target, settings: &Settings, policy: KeyPolicy, remote: &str) -> Command {
    if let Some(dir) = &settings.control_dir
        && let Err(e) = std::fs::create_dir_all(dir)
    {
        tracing::warn!(dir = %dir.display(), error = %e, "cannot create ssh control dir");
    }
    if let Some(dir) = settings.known_hosts.parent()
        && let Err(e) = std::fs::create_dir_all(dir)
    {
        tracing::warn!(dir = %dir.display(), error = %e, "cannot create config dir");
    }
    let mut cmd = Command::new("ssh");
    cmd.args(args(target, settings, policy)).arg(remote);
    tracing::debug!(host = %target.name, address = %target.address, port = target.port, ?policy, "ssh");
    cmd
}

/// Quote `s` for a POSIX shell (single quotes, embedded quotes escaped).
pub fn shell_quote(s: &str) -> String {
    if !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./=:,+@%".contains(c))
    {
        return s.to_owned();
    }
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Join words into one POSIX shell command line.
pub fn shell_join<S: AsRef<str>>(words: &[S]) -> String {
    words
        .iter()
        .map(|w| shell_quote(w.as_ref()))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Why an ssh call failed, read from its stderr (ssh exits 255 for all).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failure {
    /// The machine at that address is not the pinned host.
    HostKeyMismatch,
    /// The host key is not pinned yet.
    HostKeyUnknown,
    /// Key authentication was refused.
    AuthRefused,
    /// No connection (refused, timed out, no route, name not resolved).
    Unreachable,
    /// Anything else.
    Other,
}

/// Classify an ssh failure from its stderr.
pub fn classify_failure(stderr: &str) -> Failure {
    let s = stderr.to_ascii_lowercase();
    if s.contains("host key for") && s.contains("has changed")
        || s.contains("remote host identification has changed")
    {
        Failure::HostKeyMismatch
    } else if s.contains("host key verification failed") || s.contains("no host key is known") {
        Failure::HostKeyUnknown
    } else if s.contains("permission denied") {
        Failure::AuthRefused
    } else if [
        "connection refused",
        "timed out",
        "no route to host",
        "could not resolve hostname",
        "network is unreachable",
        "connection closed by",
        "connection reset",
    ]
    .iter()
    .any(|m| s.contains(m))
    {
        Failure::Unreachable
    } else {
        Failure::Other
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target() -> Target {
        Target {
            name: "helios".to_owned(),
            address: "192.0.2.10".to_owned(),
            port: 2222,
            user: Some("user".to_owned()),
        }
    }

    fn settings() -> Settings {
        Settings {
            known_hosts: PathBuf::from("/c/my dir/known_hosts"),
            control_dir: Some(PathBuf::from("/s/ssh")),
            connect_timeout_secs: 5,
        }
    }

    fn strings(args: &[OsString]) -> Vec<String> {
        args.iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    // frob:tests crates/goway/src/ssh.rs::command
    // frob:tests crates/goway/src/ssh.rs::Settings.from_paths
    #[test]
    fn ssh_pins_identity_by_alias_with_strict_checking() {
        let a = strings(&args(&target(), &settings(), KeyPolicy::Strict));
        assert!(a.contains(&"HostKeyAlias=goway-helios".to_owned()));
        assert!(a.contains(&"StrictHostKeyChecking=yes".to_owned()));
        assert!(a.contains(&"UserKnownHostsFile=\"/c/my dir/known_hosts\"".to_owned()));
        assert!(a.contains(&"GlobalKnownHostsFile=none".to_owned()));
        assert!(a.contains(&"BatchMode=yes".to_owned()));
        assert_eq!(
            &a[a.len() - 6..],
            ["-p", "2222", "-l", "user", "--", "192.0.2.10"]
        );

        let add = strings(&args(&target(), &settings(), KeyPolicy::AcceptNew));
        assert!(add.contains(&"StrictHostKeyChecking=accept-new".to_owned()));

        let paths = Paths {
            config_dir: PathBuf::from("/c"),
            state_dir: PathBuf::from("/s"),
        };
        let s = Settings::from_paths(&paths);
        assert_eq!(s.known_hosts, PathBuf::from("/c/known_hosts"));
        let cmd = command(&target(), &s, KeyPolicy::Strict, "uname -m");
        let all = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(all.last().map(String::as_str), Some("uname -m"));
        assert!(all.contains(&"UserKnownHostsFile=/c/known_hosts".to_owned()));
    }

    #[test]
    fn shell_quoting_round_trips_through_sh() {
        let words = ["echo", "a b", "it's", "$HOME", "", "x;y"];
        let line = shell_join(&words);
        let out = std::process::Command::new("sh")
            .args(["-c", &format!("printf '%s\\n' {}", shell_join(&words[1..]))])
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&out.stdout),
            "a b\nit's\n$HOME\n\nx;y\n"
        );
        assert!(line.starts_with("echo "));
    }

    #[test]
    fn classifies_ssh_failures() {
        let cases = [
            (
                "Host key for goway-helios has changed and you have requested strict checking.\nHost key verification failed.",
                Failure::HostKeyMismatch,
            ),
            (
                "No ED25519 host key is known for goway-q and you have requested strict checking.\nHost key verification failed.",
                Failure::HostKeyUnknown,
            ),
            (
                "user@1.2.3.4: Permission denied (publickey).",
                Failure::AuthRefused,
            ),
            (
                "ssh: connect to host 1.2.3.4 port 2222: Connection refused",
                Failure::Unreachable,
            ),
            (
                "ssh: connect to host 1.2.3.4 port 2222: Connection timed out",
                Failure::Unreachable,
            ),
            ("weird", Failure::Other),
        ];
        for (text, want) in cases {
            assert_eq!(classify_failure(text), want, "{text}");
        }
    }
}
