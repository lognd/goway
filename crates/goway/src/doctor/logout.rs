//! doctor's logout checks: what happens to goway's processes and files when
//! the last login session of the helper's user ends.
//!
//! Two risks, both read from the helper by one extra probe appended to its
//! `doctor` call: systemd-logind configured with `KillUserProcesses=yes`
//! while lingering is off (everything the user left running is killed at
//! logout), and a remote root on an encrypted or network file system, which
//! is unmounted or unreachable when nobody is logged in. The first has a
//! root fix (`loginctl enable-linger USER`, through `--rsudo`); the second
//! is a setting only the owner can choose, so it is reported with the fix
//! in words.

use std::collections::BTreeMap;

use super::{Check, Fix, Level};
use crate::ssh;

/// File system types whose contents come and go with a login or the network.
const UNSTABLE_FS: [(&str, &str); 9] = [
    ("ecryptfs", "encrypted"),
    ("fuse.ecryptfs", "encrypted"),
    ("fuse.encfs", "encrypted"),
    ("fuse.gocryptfs", "encrypted"),
    ("nfs", "a network file system"),
    ("nfs4", "a network file system"),
    ("cifs", "a network file system"),
    ("smb3", "a network file system"),
    ("fuse.sshfs", "a network file system"),
];

/// The shell script that reports the logout facts (`logind.*`, `root.fstype`).
fn probe_script(remote_root: &str) -> String {
    let root = ssh::shell_quote(remote_root);
    format!(
        "if command -v loginctl >/dev/null 2>&1; then\n\
         u=$(id -un); k=no\n\
         for f in /usr/lib/systemd/logind.conf /etc/systemd/logind.conf /usr/lib/systemd/logind.conf.d/*.conf /etc/systemd/logind.conf.d/*.conf; do\n\
         [ -r \"$f\" ] || continue\n\
         v=$(sed -n 's/^[[:space:]]*KillUserProcesses[[:space:]]*=[[:space:]]*\\([A-Za-z0-9]*\\).*/\\1/p' \"$f\" | tail -1)\n\
         [ -n \"$v\" ] && k=$v\n\
         done\n\
         l=$(loginctl show-user \"$u\" -p Linger --value 2>/dev/null || true)\n\
         if [ -z \"$l\" ] && [ -e \"/var/lib/systemd/linger/$u\" ]; then l=yes; fi\n\
         printf 'logind.user=%s\\nlogind.kill=%s\\nlogind.linger=%s\\n' \"$u\" \"$k\" \"${{l:-unknown}}\"\n\
         fi\n\
         r={root}\n\
         case \"$r\" in /*) p=$r ;; *) p=$HOME/$r ;; esac\n\
         while [ ! -e \"$p\" ] && [ \"$p\" != / ]; do p=$(dirname \"$p\"); done\n\
         t=$(findmnt -n -o FSTYPE -T \"$p\" 2>/dev/null | head -1)\n\
         printf 'root.fstype=%s\\n' \"$t\"\n"
    )
}

/// `base` (the `doctor` call) followed by the logout probe for `remote_root`.
pub(super) fn wrap(base: &str, remote_root: &str) -> String {
    super::append_script(base, &probe_script(remote_root))
}

/// A systemd boolean as the probe reports it.
fn is_yes(value: &str) -> bool {
    matches!(
        value.to_ascii_lowercase().as_str(),
        "yes" | "true" | "1" | "on"
    )
}

/// A login name safe to put in a command (`loginctl enable-linger NAME`).
fn valid_user(name: &str) -> bool {
    let b = name.as_bytes();
    !b.is_empty()
        && b.len() <= 32
        && (b[0].is_ascii_lowercase() || b[0] == b'_')
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || b"_-".contains(c))
}

/// The logout checks from a host's facts; none for a host that did not answer the probe.
pub(super) fn checks(facts: &BTreeMap<String, String>) -> Vec<Check> {
    let mut out = Vec::new();
    if let (Some(kill), Some(user)) = (facts.get("logind.kill"), facts.get("logind.user")) {
        let lingering = facts.get("logind.linger").is_some_and(|l| is_yes(l));
        let name = "logout";
        out.push(if is_yes(kill) && !lingering {
            Check {
                name: name.to_owned(),
                explain: Some(
                    "systemd-logind kills a user's processes when the last session ends (KillUserProcesses=yes) and lingering is off, so anything goway leaves running there, including its keepalives and a run whose ssh connection drops, dies at logout. `loginctl enable-linger USER` keeps the user's processes (as root; goway runs it only with --fix --rsudo and your confirmation). The setting was read from logind.conf and its drop-ins, so a unit override elsewhere can differ."
                        .to_owned(),
                ),
                level: Level::Warn,
                detail: format!(
                    "logind kills {user}'s processes at logout (KillUserProcesses=yes) and lingering is off; fix: loginctl enable-linger {user}"
                ),
                fix: valid_user(user).then(|| Fix {
                    command: format!("loginctl enable-linger {user}"),
                    root: true,
                    why: format!(
                        "logind would kill {user}'s processes at logout; lingering keeps them (undo: loginctl disable-linger {user})"
                    ),
                }),
            }
        } else {
            Check {
                name: name.to_owned(),
                explain: None,
                level: Level::Ok,
                detail: if lingering {
                    "lingering is on for this user".to_owned()
                } else {
                    "logind keeps user processes at logout".to_owned()
                },
                fix: None,
            }
        });
    }
    if let Some(fstype) = facts.get("root.fstype").filter(|t| !t.is_empty()) {
        let unstable = UNSTABLE_FS.iter().find(|(t, _)| t == fstype);
        out.push(match unstable {
            Some((_, kind)) => Check {
                name: "remote root file system".to_owned(),
                explain: Some(
                    "goway keeps its work trees, caches and locks under remote_root. On an encrypted home (ecryptfs, encfs) the directory is only mounted while its owner is logged in; on a network file system it vanishes with the network. Set defaults.remote_root in goway's config to a directory on an ordinary local disk, such as /srv/goway (create it first and give your user ownership)."
                        .to_owned(),
                ),
                level: Level::Warn,
                detail: format!(
                    "{fstype}: goway's remote root is on {kind} storage that is not there when nobody is logged in; set defaults.remote_root to a local directory such as /srv/goway"
                ),
                fix: None,
            },
            None => Check {
                name: "remote root file system".to_owned(),
                explain: None,
                level: Level::Ok,
                detail: fstype.clone(),
                fix: None,
            },
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    // frob:tests crates/goway/src/doctor/logout.rs::checks
    #[test]
    fn killing_logind_without_lingering_warns_with_a_root_fix() {
        let c = checks(&facts(&[
            ("logind.user", "builder"),
            ("logind.kill", "yes"),
            ("logind.linger", "no"),
        ]));
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].level, Level::Warn);
        let fix = c[0].fix.as_ref().unwrap();
        assert!(fix.root);
        assert_eq!(fix.command, "loginctl enable-linger builder");
        assert!(c[0].detail.contains("enable-linger"), "{}", c[0].detail);
    }

    #[test]
    fn lingering_or_a_keeping_logind_passes_and_a_hostile_user_gets_no_command() {
        for (kill, linger) in [("yes", "yes"), ("no", "no"), ("True", "1")] {
            let c = checks(&facts(&[
                ("logind.user", "builder"),
                ("logind.kill", kill),
                ("logind.linger", linger),
            ]));
            let expected = if is_yes(kill) && !is_yes(linger) {
                Level::Warn
            } else {
                Level::Ok
            };
            assert_eq!(c[0].level, expected, "{kill} {linger}");
        }
        let c = checks(&facts(&[
            ("logind.user", "a; rm -rf ~"),
            ("logind.kill", "yes"),
            ("logind.linger", "no"),
        ]));
        assert_eq!(c[0].level, Level::Warn);
        assert!(c[0].fix.is_none());
        assert!(checks(&BTreeMap::new()).is_empty());
    }

    #[test]
    fn an_encrypted_or_network_root_warns_and_an_ordinary_one_passes() {
        for fs in ["ecryptfs", "nfs4", "cifs"] {
            let c = checks(&facts(&[("root.fstype", fs)]));
            assert_eq!(c[0].level, Level::Warn, "{fs}");
            assert!(c[0].detail.contains("remote_root"), "{}", c[0].detail);
        }
        let c = checks(&facts(&[("root.fstype", "ext4")]));
        assert_eq!(c[0].level, Level::Ok);
        assert!(checks(&facts(&[("root.fstype", "")])).is_empty());
    }

    #[test]
    fn the_probe_is_valid_shell_that_quotes_the_remote_root() {
        let script = probe_script(".cache/goway; touch pwned");
        assert!(script.contains("r='.cache/goway; touch pwned'"), "{script}");
        let ok = std::process::Command::new("bash")
            .args(["-n", "-c", &script])
            .status()
            .map(|s| s.success());
        assert_ne!(ok.ok(), Some(false));
        let line = wrap("base", "x/goway");
        assert!(line.starts_with("base; bash -c 'eval "), "{line}");
        assert!(!line.contains('\n') && !line.contains('\\') && !line.contains('!'));
    }
}
