//! Typed errors of goway itself and the exit codes they map to.
//!
//! The exit code contract (frob records it as evidence): a remote command's
//! own exit code passes through unchanged, 128+N when it died of signal N,
//! and [`EXIT_GOWAY_FAILURE`] when goway could not run the command at all.

use std::path::PathBuf;

/// Exit code when goway itself fails, borrowed from `docker run` (125).
pub const EXIT_GOWAY_FAILURE: u8 = 125;

/// Every failure goway can report; each variant renders as one clear message.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A file goway owns could not be read or written.
    #[error("cannot {action} {path}: {source}")]
    Io {
        /// What goway tried to do, such as "read" or "create".
        action: &'static str,
        /// The path involved.
        path: PathBuf,
        /// The underlying OS error.
        source: std::io::Error,
    },
    /// goway was started by goway too many levels deep.
    #[error(
        "goway is nested {depth} deep (limit {limit}): {chain}; a command that calls goway recursively is refused here"
    )]
    TooDeep {
        /// The depth this goway saw in `GOWAY_DEPTH`.
        depth: u32,
        /// The nesting limit.
        limit: u32,
        /// The machines the chain of goway runs passed through.
        chain: String,
    },
    /// The config file is malformed or inconsistent.
    #[error("config {path}: {message}")]
    Config {
        /// The config file.
        path: PathBuf,
        /// What is wrong, from the parser or validation.
        message: String,
    },
    /// The cached host state file is malformed.
    #[error("state {path}: {message}")]
    State {
        /// The state file.
        path: PathBuf,
        /// What is wrong.
        message: String,
    },
    /// No configured host has this name.
    #[error("no host named `{0}` in the config; add it with `goway host add {0}`")]
    UnknownHost(String),
    /// No candidate address answered with the host's pinned key.
    #[error("cannot reach host `{name}`{}", render_misses(misses))]
    HostNotFound {
        /// The host name.
        name: String,
        /// Every candidate tried and why it failed.
        misses: Vec<crate::resolve::Miss>,
    },
    /// `goway host add` found no usable Linux sshd for the host.
    #[error("cannot add host `{name}`: {reason}")]
    HostAdd {
        /// The host name.
        name: String,
        /// Why, with the next step.
        reason: String,
    },
    /// git failed or the directory is not a work tree.
    #[error("{message}")]
    Git {
        /// git's complaint or why git could not run.
        message: String,
    },
    /// A remote call (ssh, or PowerShell through WSL interop) to a resolved host failed.
    #[error("remote call to `{host}` failed: {message}")]
    Ssh {
        /// The host name.
        host: String,
        /// stderr of ssh or the remote script.
        message: String,
    },
    /// No host of the pool can take a job now.
    #[error("no usable host:\n  {}\n  next: `goway status` shows each host's state; `goway add NAME` adds another helper; `goway run --host local -- CMD` runs on this machine, and `[local] fallback = true` in the config does that automatically when no helper is reachable", .0.join("\n  "))]
    NoHost(Vec<String>),
    /// Hosts answered, but none meets the run's `--needs`.
    #[error("no host meets the requirements:\n  {}\n  next: `goway status` shows each host's hardware; relax --needs, or label/add a host that fits", .0.join("\n  "))]
    NeedsUnmet(Vec<String>),
    /// The command line is inconsistent or incomplete.
    #[error("{0}")]
    Usage(String),
    /// The host cannot say for certain what runs in place of the command's program.
    #[error(
        "`{requested}` has no certain equivalent on this {os} host ({why}); goway does not run a guess. \
         Name a program that host has, add a [translate] entry to goway.toml, or run on a host of this machine's OS"
    )]
    TranslationDoubt {
        /// The program the command names.
        requested: String,
        /// The `[translate]` key of the host's OS.
        os: &'static str,
        /// Why the host could not say.
        why: String,
        /// Whether an unpinned run may pick a host of this machine's OS instead of stopping.
        repickable: bool,
    },
    /// A command line feature that is planned but not built yet.
    #[error("`{0}` is not implemented yet")]
    NotImplemented(&'static str),
}

fn render_misses(misses: &[crate::resolve::Miss]) -> String {
    use crate::ssh::Failure;
    if misses.is_empty() {
        return ": no address found for it (no cached address, no `address` in the config, and neither the name nor NAME.local resolved)\n  next: is it switched on, awake and on the same network? check with `goway status`\n  next: if it is on and nearby, this network may block name discovery (mDNS) between devices; give its address in the config (`address = \"192.0.2.10\"`) or use a network where devices can see each other (details in docs/troubleshooting.md, \"A network that hides the helpers\")".to_owned();
    }
    let mut out = String::from("; tried:");
    for m in misses {
        out.push_str("\n  ");
        out.push_str(&m.to_string());
    }
    let any = |f: Failure| misses.iter().any(|m| m.failure == f);
    if any(Failure::HostKeyUnknown) {
        out.push_str("\n  next: pin its key first: `goway add NAME` (or `goway host add NAME`)");
    }
    if any(Failure::AuthRefused) {
        out.push_str("\n  next: set up key login: `goway add NAME` (asks for its password once)");
        out.push_str("\n  next: ");
        out.push_str(&permission_denied_hint("USER"));
    }
    if any(Failure::Throttled) {
        out.push_str("\n  next: goway counts the failed logins it causes and stops at its own limit so the helper's fail2ban never bans this machine; wait for the time above, then retry (details in docs/troubleshooting.md)");
    }
    if any(Failure::ProbableBan) {
        out.push_str("\n  next: the helper refused the connection right after failed logins: this machine is probably banned by fail2ban or sshguard. On the helper: `sudo fail2ban-client status sshd` lists banned addresses, `sudo fail2ban-client set sshd unbanip ADDRESS` lifts one (ADDRESS is this machine's address as the helper sees it)");
    }
    if any(Failure::HostKeyMismatch) {
        out.push_str("\n  next: that address answered with a different key: another machine has it now (goway skips it), or NAME was reinstalled; if reinstalled: `goway host remove NAME`, then `goway add NAME`");
    }
    if misses.iter().all(|m| m.failure == Failure::Unreachable) {
        out.push_str("\n  next: is it switched on, awake and on the same network, with WSL running? check with `goway status`");
    }
    out.push_str(&network_hints(misses));
    out
}

/// Hints for the two network-shaped failures: a name that resolves to an
/// address nothing answers on, and two machines answering for one name.
fn network_hints(misses: &[crate::resolve::Miss]) -> String {
    use crate::resolve::Source;
    use crate::ssh::Failure;
    use std::fmt::Write as _;
    let looked_up = |m: &&crate::resolve::Miss| {
        matches!(m.source, Source::Name | Source::Mdns | Source::WindowsMdns)
    };
    let mut out = String::new();
    let hidden = misses
        .iter()
        .filter(looked_up)
        .any(|m| m.failure == Failure::Unreachable);
    if hidden {
        out.push_str("\n  next: the name resolved but nothing answered at that address: this looks like a network that isolates devices from each other (guest Wi-Fi client isolation) or a VPN that routes only some addresses; join a network where devices can see each other, or leave the VPN's full-tunnel mode, or put a reachable address in the config (`address = \"192.0.2.10\"`)");
    }
    let mut answering: Vec<&str> = misses
        .iter()
        .filter(looked_up)
        .filter(|m| {
            matches!(
                m.failure,
                Failure::HostKeyMismatch | Failure::HostKeyUnknown
            )
        })
        .map(|m| m.address.as_str())
        .collect();
    answering.sort_unstable();
    answering.dedup();
    if answering.len() >= 2 {
        let _ = write!(
            out,
            "\n  next: {} different machines answered for this name without its pinned key ({}); goway never uses an address whose key it has not confirmed. Rename one machine so the names differ, or put the right address in the config (`address = \"192.0.2.10\"`)",
            answering.len(),
            answering.join(", ")
        );
    }
    out
}

/// The likely causes of "Permission denied" for `user`, each with the command that checks it.
pub fn permission_denied_hint(user: &str) -> String {
    format!(
        "Permission denied means the helper refused the login; the usual causes, with a check on the helper for each:\n    \
         - no password is set for {user} (common with automatic login): `passwd -S {user}` shows NP, and sshd never accepts an empty password; add goway's key by hand (`goway add NAME --no-password` prints the commands) or set one with `passwd`\n    \
         - a mistyped password: type it again slowly, or use `--no-password`\n    \
         - password login is switched off: `sudo sshd -T | grep -i passwordauthentication` says no; add the key by hand\n    \
         - the wrong user: pass the helper's Linux account with `--user`\n    \
         - a lockout or a ban after failed attempts: `faillock --user {user}` (reset with `faillock --user {user} --reset`), `sudo fail2ban-client status sshd`\n    \
         - AllowUsers or AllowGroups excludes {user}: `sudo sshd -T | grep -iE 'allowusers|allowgroups'`\n    \
         details: docs/troubleshooting.md, \"goway add says Permission denied\""
    )
}

impl Error {
    /// The process exit code for this error; always [`EXIT_GOWAY_FAILURE`].
    pub fn exit_code(&self) -> u8 {
        EXIT_GOWAY_FAILURE
    }

    /// Build an [`Error::Io`] for `action` on `path`.
    pub fn io(action: &'static str, path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            action,
            path: path.into(),
            source,
        }
    }
}

/// goway's result alias.
pub type Result<T, E = Error> = std::result::Result<T, E>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_permission_denied_hint_names_every_cause_with_its_check() {
        let hint = permission_denied_hint("alice");
        for needle in [
            "passwd -S alice",
            "NP",
            "mistyped",
            "sshd -T | grep -i passwordauthentication",
            "--user",
            "faillock --user alice",
            "fail2ban-client status sshd",
            "AllowUsers",
            "AllowGroups",
            "--no-password",
        ] {
            assert!(hint.contains(needle), "missing {needle}: {hint}");
        }
    }

    // frob:ticket 01M42BHTGMABJE3ZAPBZQETW47
    // frob:tests crates/goway/src/error.rs::network_hints
    #[test]
    fn hidden_helpers_and_duplicate_names_get_their_own_hint() {
        use crate::resolve::{Miss, Source};
        use crate::ssh::Failure;
        let miss = |address: &str, source, failure| Miss {
            address: address.to_owned(),
            source,
            failure,
            detail: String::new(),
        };
        let isolated = render_misses(&[miss("192.0.2.5", Source::Mdns, Failure::Unreachable)]);
        assert!(isolated.contains("isolates devices"), "{isolated}");
        let cached_only = render_misses(&[miss("192.0.2.5", Source::Cached, Failure::Unreachable)]);
        assert!(!cached_only.contains("isolates devices"), "{cached_only}");
        let twins = render_misses(&[
            miss("192.0.2.5", Source::Name, Failure::HostKeyMismatch),
            miss("192.0.2.6", Source::Mdns, Failure::HostKeyUnknown),
        ]);
        assert!(twins.contains("2 different machines"), "{twins}");
        assert!(twins.contains("never uses an address"), "{twins}");
        assert!(render_misses(&[]).contains("may block name discovery"));
    }

    #[test]
    fn every_goway_error_exits_125() {
        let errors = [
            Error::NotImplemented("gc"),
            Error::UnknownHost("q".to_owned()),
            Error::NeedsUnmet(vec!["h: lacks gpu".to_owned()]),
            Error::io("read", "/x", std::io::Error::other("boom")),
        ];
        for e in errors {
            assert_eq!(e.exit_code(), 125, "{e}");
        }
    }

    // frob:ticket 01M42FEZBPMJ958RA4K1TD3D5H
    #[test]
    fn the_error_for_an_interop_host_names_the_transport_not_ssh() {
        let e = Error::Ssh {
            host: "helios".to_owned(),
            message: "WSL interop: powershell.exe failed".to_owned(),
        };
        let text = e.to_string();
        assert!(!text.contains("ssh to"), "{text}");
        assert!(text.contains("WSL interop"), "{text}");
    }
}
