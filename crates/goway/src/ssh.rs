//! Building ssh invocations. goway spawns the system `ssh` so the user's
//! agent, config and the Windows OpenSSH client all just work.
//!
//! Identity is pinned per host: every call uses `HostKeyAlias=goway-<name>`
//! with goway's own `known_hosts`, so the key is bound to the name and the
//! address is free to change. A wrong address fails the host key check.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

pub mod attempts;
pub mod mux;

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
    /// A private key to offer, if not left to ssh config.
    pub identity: Option<PathBuf>,
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
        attempts::init(&paths.state_dir);
        let control_dir = cfg!(unix)
            .then(|| paths.control_dir())
            .filter(|d| {
                let fits = control_path_fits(d);
                if !fits {
                    tracing::info!(dir = %d.display(), "control socket path too long; ssh multiplexing off");
                }
                fits
            });
        Self {
            known_hosts: paths.known_hosts(),
            control_dir,
            connect_timeout_secs: 5,
        }
    }
}

/// Unix socket paths are limited to 108 bytes; `%C` expands to 40 hex
/// characters and ssh appends a temporary suffix while binding.
pub fn control_path_fits(dir: &std::path::Path) -> bool {
    dir.as_os_str().len() + 1 + 40 + 20 < 108
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
    let socket = settings.control_dir.as_ref().map(|d| d.join("%C"));
    args_with(target, settings, policy, socket.as_deref())
}

/// [`args`] with an explicit control socket (`None`: no multiplexing).
fn args_with(
    target: &Target,
    settings: &Settings,
    policy: KeyPolicy,
    socket: Option<&Path>,
) -> Vec<OsString> {
    let strict = match policy {
        KeyPolicy::Strict => "yes",
        KeyPolicy::AcceptNew => "accept-new",
    };
    let mut opts = vec![
        "BatchMode=yes".to_owned(),
        // Key login only, never a password prompt: a failed attempt counts
        // toward fail2ban and sshguard limits (`allow_password` relaxes this
        // for the one deliberate password login of `goway add`).
        "PreferredAuthentications=publickey".to_owned(),
        "NumberOfPasswordPrompts=0".to_owned(),
        format!("HostKeyAlias={}", key_alias(&target.name)),
        format!("StrictHostKeyChecking={strict}"),
        format!("UserKnownHostsFile={}", option_value(&settings.known_hosts)),
        "GlobalKnownHostsFile=none".to_owned(),
        "HashKnownHosts=no".to_owned(),
        format!("ConnectTimeout={}", settings.connect_timeout_secs),
        "ServerAliveInterval=15".to_owned(),
        "ServerAliveCountMax=4".to_owned(),
        "LogLevel=ERROR".to_owned(),
        // Whatever the user's ssh config says, a build host gets no agent,
        // no X11, no tunnels, no delegated credentials, and cannot rewrite
        // goway's known_hosts.
        "ForwardAgent=no".to_owned(),
        "ForwardX11=no".to_owned(),
        "ClearAllForwardings=yes".to_owned(),
        "GSSAPIDelegateCredentials=no".to_owned(),
        "PermitLocalCommand=no".to_owned(),
        "UpdateHostKeys=no".to_owned(),
        "VerifyHostKeyDNS=no".to_owned(),
        "CheckHostIP=no".to_owned(),
    ];
    if let Some(socket) = socket {
        opts.push("ControlMaster=auto".to_owned());
        opts.push(format!("ControlPath={}", option_value(socket)));
        opts.push("ControlPersist=60s".to_owned());
    } else {
        // Off means off: never reuse a connection from the user's config
        // (a key-only check must not ride a password-authenticated master).
        opts.push("ControlMaster=no".to_owned());
        opts.push("ControlPath=none".to_owned());
    }
    let mut out: Vec<OsString> = Vec::new();
    for o in opts {
        out.push("-o".into());
        out.push(o.into());
    }
    if let Some(identity) = &target.identity {
        out.push("-o".into());
        out.push(format!("IdentityFile={}", option_value(identity)).into());
        out.push("-o".into());
        out.push("IdentitiesOnly=yes".into());
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

/// Create the control socket directory and check it is private to this
/// user: a real directory (not a symlink), mode 0700, owned by the same user
/// as the home directory. Anything else could let another account host the
/// `ControlPath` sockets, so multiplexing is turned off instead.
fn prepare_control_dir(dir: &Path) -> bool {
    if let Err(e) = std::fs::create_dir_all(dir) {
        tracing::warn!(dir = %dir.display(), error = %e, "cannot create ssh control dir");
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
        let Ok(meta) = std::fs::symlink_metadata(dir) else {
            return false;
        };
        if !meta.is_dir() {
            return false;
        }
        if meta.permissions().mode() & 0o777 != 0o700
            && std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).is_err()
        {
            return false;
        }
        let me = dirs::home_dir().and_then(|h| std::fs::metadata(h).ok());
        let mode_ok =
            std::fs::symlink_metadata(dir).is_ok_and(|m| m.permissions().mode() & 0o777 == 0o700);
        mode_ok && me.is_some_and(|h| h.uid() == meta.uid())
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// An ssh command running `remote` (a shell command line) on `target`.
pub fn command(target: &Target, settings: &Settings, policy: KeyPolicy, remote: &str) -> Command {
    let checked;
    let settings = match &settings.control_dir {
        Some(dir) if !prepare_control_dir(dir) => {
            tracing::warn!(dir = %dir.display(), "ssh control dir is not private to this user; multiplexing off");
            checked = Settings {
                control_dir: None,
                ..settings.clone()
            };
            &checked
        }
        _ => settings,
    };
    if let Some(dir) = settings.known_hosts.parent()
        && let Err(e) = std::fs::create_dir_all(dir)
    {
        tracing::warn!(dir = %dir.display(), error = %e, "cannot create config dir");
    }
    // One of a few masters per host, claimed per process, so a wave of
    // runs never exceeds the helper's MaxSessions; none free: no mux.
    let socket = settings
        .control_dir
        .as_deref()
        .and_then(|dir| mux::socket(dir, target));
    let mut cmd = Command::new("ssh");
    cmd.args(args_with(target, settings, policy, socket.as_deref()))
        .arg(remote);
    scrub_env(&mut cmd);
    tracing::debug!(host = %target.name, address = %target.address, port = target.port, ?policy, "ssh");
    cmd
}

/// How ssh names the host in its password prompt: ssh shows the `HostKeyAlias`,
/// so the prompt would read `user@goway-NAME` unless the alias is swapped for
/// the name the user typed, with a `known_hosts` copy keyed by that name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PasswordPrompt {
    /// The name shown in ssh's prompt (what the user typed).
    pub shown: String,
    /// A `known_hosts` file whose entries are keyed by `shown`.
    pub known_hosts: PathBuf,
}

impl PasswordPrompt {
    /// Copy the pinned entries of `name` from `source` to `dest` keyed by the plain name.
    ///
    /// # Errors
    ///
    /// The I/O error when `dest` cannot be written; a missing `source` gives an empty file.
    pub fn create(name: &str, source: &Path, dest: &Path) -> std::io::Result<Self> {
        let alias = key_alias(name);
        let text = std::fs::read_to_string(source).unwrap_or_default();
        let mut out = String::new();
        for line in text.lines() {
            if let Some(rest) = line.strip_prefix(&format!("{alias} ")) {
                out.push_str(name);
                out.push(' ');
                out.push_str(rest);
                out.push('\n');
            }
        }
        std::fs::write(dest, out)?;
        Ok(Self {
            shown: name.to_owned(),
            known_hosts: dest.to_owned(),
        })
    }
}

/// Let `cmd` (built by [`command`]) ask for a password exactly once:
/// `BatchMode=no`, password methods only (no agent keys are offered), one
/// prompt, so a wrong password is one failed attempt and never a ban. With
/// `prompt`, the host is shown under the name the user typed.
pub fn allow_password(cmd: &mut Command, prompt: Option<&PasswordPrompt>) {
    let args: Vec<OsString> =
        cmd.get_args()
            .map(|a| {
                let text = a.to_string_lossy();
                match (&*text, prompt) {
                    ("BatchMode=yes", _) => OsString::from("BatchMode=no"),
                    ("PreferredAuthentications=publickey", _) => {
                        OsString::from("PreferredAuthentications=password,keyboard-interactive")
                    }
                    ("NumberOfPasswordPrompts=0", _) => OsString::from("NumberOfPasswordPrompts=1"),
                    (t, Some(p)) if t.starts_with("HostKeyAlias=") => {
                        OsString::from(format!("HostKeyAlias={}", p.shown))
                    }
                    (t, Some(p)) if t.starts_with("UserKnownHostsFile=") => OsString::from(
                        format!("UserKnownHostsFile={}", option_value(&p.known_hosts)),
                    ),
                    _ => OsString::from(a),
                }
            })
            .collect();
    let mut rebuilt = Command::new(cmd.get_program());
    rebuilt.args(args);
    scrub_env(&mut rebuilt);
    *cmd = rebuilt;
}

/// Make `cmd` (built by [`command`]) allocate a remote tty, for commands
/// that must prompt, such as sudo.
pub fn force_tty(cmd: &mut Command) {
    let args: Vec<OsString> = cmd.get_args().map(OsString::from).collect();
    let mut rebuilt = Command::new(cmd.get_program());
    rebuilt.arg("-t").args(args);
    scrub_env(&mut rebuilt);
    *cmd = rebuilt;
}

/// The environment ssh itself needs; nothing else reaches it, so no local
/// variable can travel to a host through a `SendEnv` in the user's config.
const SSH_ENV: &[&str] = &[
    "HOME",
    "USER",
    "LOGNAME",
    "PATH",
    "SHELL",
    "TERM",
    "LANG",
    "LC_ALL",
    "LC_CTYPE",
    "SSH_AUTH_SOCK",
    "XDG_RUNTIME_DIR",
    "TMPDIR",
    // Windows OpenSSH.
    "SYSTEMROOT",
    "SYSTEMDRIVE",
    "WINDIR",
    "COMSPEC",
    "PATHEXT",
    "USERPROFILE",
    "USERNAME",
    "USERDOMAIN",
    "HOMEDRIVE",
    "HOMEPATH",
    "APPDATA",
    "LOCALAPPDATA",
    "PROGRAMDATA",
    "TEMP",
    "TMP",
];

/// Clear `cmd`'s environment down to [`SSH_ENV`], plus the comma-separated
/// names in `GOWAY_SSH_PASS_ENV` (for test harnesses with a fake ssh).
pub fn scrub_env(cmd: &mut Command) {
    cmd.env_clear();
    let extra = std::env::var("GOWAY_SSH_PASS_ENV").unwrap_or_default();
    let extra = extra.split(',').map(str::trim).filter(|s| !s.is_empty());
    for name in SSH_ENV.iter().copied().chain(extra) {
        if let Some(value) = std::env::var_os(name) {
            cmd.env(name, value);
        }
    }
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
    /// goway did not try: too many failed logins to this host lately
    /// (see [`attempts`]).
    Throttled,
    /// The connection was refused soon after failed logins: probably a
    /// fail2ban or sshguard ban of this machine.
    ProbableBan,
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
            identity: None,
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

    #[cfg(unix)]
    #[test]
    fn a_control_dir_that_is_not_private_turns_multiplexing_off() {
        use std::os::unix::fs::PermissionsExt as _;
        let tmp = tempfile::tempdir().unwrap();
        let mux = |dir: &Path| {
            let s = Settings {
                control_dir: Some(dir.to_owned()),
                ..settings()
            };
            let cmd = command(&target(), &s, KeyPolicy::Strict, "true");
            let a: Vec<String> = cmd
                .get_args()
                .map(|a| a.to_string_lossy().into_owned())
                .collect();
            a.contains(&"ControlMaster=auto".to_owned())
        };
        // A fresh directory is made private and used, even if it was loose.
        let ok = tmp.path().join("ok");
        std::fs::create_dir(&ok).unwrap();
        std::fs::set_permissions(&ok, std::fs::Permissions::from_mode(0o777)).unwrap();
        assert!(mux(&ok));
        assert_eq!(
            std::fs::metadata(&ok).unwrap().permissions().mode() & 0o777,
            0o700
        );
        // A symlink (to a directory someone else may control) is refused.
        let real = tmp.path().join("real");
        std::fs::create_dir(&real).unwrap();
        let link = tmp.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        assert!(!mux(&link));
        // So is a plain file in its place.
        let file = tmp.path().join("file");
        std::fs::write(&file, "x").unwrap();
        assert!(!mux(&file));
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
        for hardened in [
            "ForwardAgent=no",
            "ForwardX11=no",
            "ClearAllForwardings=yes",
            "UpdateHostKeys=no",
            "PermitLocalCommand=no",
        ] {
            assert!(a.contains(&hardened.to_owned()), "{hardened}");
        }
        assert_eq!(
            &a[a.len() - 6..],
            ["-p", "2222", "-l", "user", "--", "192.0.2.10"]
        );

        let add = strings(&args(&target(), &settings(), KeyPolicy::AcceptNew));
        assert!(add.contains(&"StrictHostKeyChecking=accept-new".to_owned()));

        let paths = Paths {
            config_dir: PathBuf::from("/c"),
            state_dir: PathBuf::from("/s"),
            runtime_dir: None,
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
    fn long_control_dirs_disable_multiplexing() {
        assert!(control_path_fits(std::path::Path::new(
            "/run/user/1000/goway"
        )));
        let long = PathBuf::from(format!("/tmp/{}", "x".repeat(60)));
        assert!(!control_path_fits(&long));
        let paths = Paths {
            config_dir: PathBuf::from("/c"),
            state_dir: long,
            runtime_dir: None,
        };
        assert_eq!(Settings::from_paths(&paths).control_dir, None);
    }

    #[test]
    fn ssh_gets_only_the_allowlisted_environment() {
        let cmd = command(&target(), &settings(), KeyPolicy::Strict, "true");
        let names: Vec<String> = cmd
            .get_envs()
            .filter(|(_, v)| v.is_some())
            .map(|(k, _)| k.to_string_lossy().into_owned())
            .collect();
        assert!(
            names.iter().all(|n| SSH_ENV.contains(&n.as_str())
                || std::env::var("GOWAY_SSH_PASS_ENV").is_ok_and(|e| e.contains(n.as_str()))),
            "{names:?}"
        );
        let cleared = cmd.get_envs().count() >= names.len();
        assert!(cleared);
    }

    #[cfg(unix)]
    #[test]
    fn control_dir_is_private_and_off_means_no_reuse() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let control = dir.path().join("ssh");
        let mut s = settings();
        s.control_dir = Some(control.clone());
        let _ = command(&target(), &s, KeyPolicy::Strict, "true");
        assert_eq!(
            std::fs::metadata(&control).unwrap().permissions().mode() & 0o777,
            0o700
        );
        s.control_dir = None;
        let a = strings(&args(&target(), &s, KeyPolicy::Strict));
        assert!(
            a.contains(&"ControlPath=none".to_owned())
                && a.contains(&"ControlMaster=no".to_owned())
        );
    }

    #[test]
    fn force_tty_prepends_t() {
        let mut cmd = command(&target(), &settings(), KeyPolicy::Strict, "sudo true");
        force_tty(&mut cmd);
        let all: Vec<String> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(all[0], "-t");
        assert_eq!(all.last().map(String::as_str), Some("sudo true"));
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

    // frob:tests crates/goway/src/ssh.rs::allow_password
    #[test]
    fn a_password_login_asks_once_under_the_name_the_user_typed() {
        let build = || {
            let mut cmd = Command::new("ssh");
            cmd.args(args(&target(), &settings(), KeyPolicy::Strict));
            cmd
        };
        let shown = |cmd: &Command| -> Vec<String> {
            cmd.get_args()
                .map(|a| a.to_string_lossy().into_owned())
                .collect()
        };
        let keys_only = shown(&build());
        assert!(keys_only.contains(&"NumberOfPasswordPrompts=0".to_owned()));
        assert!(keys_only.contains(&"PreferredAuthentications=publickey".to_owned()));

        let mut cmd = build();
        let prompt = PasswordPrompt {
            shown: "helios".to_owned(),
            known_hosts: PathBuf::from("/tmp/prompt hosts"),
        };
        allow_password(&mut cmd, Some(&prompt));
        let args = shown(&cmd);
        for want in [
            "BatchMode=no",
            "NumberOfPasswordPrompts=1",
            "PreferredAuthentications=password,keyboard-interactive",
            "HostKeyAlias=helios",
        ] {
            assert!(args.contains(&want.to_owned()), "{want} in {args:?}");
        }
        assert!(!args.iter().any(|a| a.contains("goway-helios")), "{args:?}");
        assert!(
            args.iter()
                .any(|a| a.starts_with("UserKnownHostsFile=") && a.contains("prompt hosts"))
        );
    }

    // frob:tests crates/goway/src/ssh.rs::PasswordPrompt
    #[test]
    fn the_prompt_known_hosts_keeps_only_this_hosts_entries_under_its_plain_name() {
        let dir = tempfile::tempdir().unwrap();
        let (src, dst) = (dir.path().join("kh"), dir.path().join("prompt"));
        std::fs::write(
            &src,
            "goway-helios ssh-ed25519 AAAA\ngoway-other ssh-ed25519 BBBB\n",
        )
        .unwrap();
        PasswordPrompt::create("helios", &src, &dst).unwrap();
        assert_eq!(
            std::fs::read_to_string(&dst).unwrap(),
            "helios ssh-ed25519 AAAA\n"
        );
    }
}
