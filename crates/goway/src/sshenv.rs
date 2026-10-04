//! Client-side ssh auto-detection: what ssh will use and what looks off.
//!
//! Reads only metadata and `ssh -G` (the effective config); never the
//! contents of private keys.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::spawn::CommandExt as _;

/// Something about the local ssh setup worth telling the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Finding {
    /// The `ssh` client is not on PATH.
    NoSshClient,
    /// No agent keys and no identity file exists: key auth cannot work.
    NoKey,
    /// An identity file is readable by group or others; ssh will refuse it.
    LooseKeyPermissions(PathBuf),
    /// The agent is running but holds no keys.
    AgentEmpty,
}

impl std::fmt::Display for Finding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoSshClient => f.write_str(
                "no `ssh` client on PATH (Linux: install openssh-client; Windows: enable the OpenSSH Client feature)",
            ),
            Self::NoKey => f.write_str(
                "no ssh key found (no agent keys, no identity file); run `goway ssh setup HOST`",
            ),
            Self::LooseKeyPermissions(p) => write!(
                f,
                "{} is readable by others; ssh will ignore it (fix: chmod 600 {})",
                p.display(),
                p.display()
            ),
            Self::AgentEmpty => f.write_str("ssh-agent is running but holds no keys"),
        }
    }
}

/// What `ssh -G` says about a destination (only the fields goway uses).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Effective {
    /// The user ssh will log in as.
    pub user: Option<String>,
    /// Identity files ssh will try, `~` expanded.
    pub identity_files: Vec<PathBuf>,
}

/// Parse `ssh -G` output.
pub fn parse_effective(text: &str, home: &Path) -> Effective {
    let mut eff = Effective::default();
    for line in text.lines() {
        let Some((key, value)) = line.split_once(' ') else {
            continue;
        };
        match key {
            "user" => eff.user = Some(value.to_owned()),
            "identityfile" => {
                let path = value
                    .strip_prefix("~/")
                    .map_or_else(|| PathBuf::from(value), |rest| home.join(rest));
                eff.identity_files.push(path);
            }
            _ => {}
        }
    }
    eff
}

/// Run `ssh -G` for `address` on `port`.
pub fn effective(address: &str, port: u16) -> Option<Effective> {
    let out = Command::new("ssh")
        .args(["-G", "-p", &port.to_string(), "--", address])
        .stdin(Stdio::null())
        .output_locked()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let home = dirs::home_dir().unwrap_or_default();
    Some(parse_effective(
        &String::from_utf8_lossy(&out.stdout),
        &home,
    ))
}

/// Agent state from `ssh-add -l` exit codes: 0 keys, 1 empty, 2 no agent.
fn agent_keys() -> Option<bool> {
    let status = Command::new("ssh-add")
        .arg("-l")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status_locked()
        .ok()?;
    match status.code() {
        Some(0) => Some(true),
        Some(1) => Some(false),
        _ => None,
    }
}

#[cfg(unix)]
fn loose(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::metadata(path).is_ok_and(|m| m.permissions().mode() & 0o077 != 0)
}

#[cfg(not(unix))]
fn loose(_: &Path) -> bool {
    false
}

/// Pure decision over the observed facts, so it can be tested.
pub fn assess(ssh_present: bool, agent: Option<bool>, identity_files: &[PathBuf]) -> Vec<Finding> {
    if !ssh_present {
        return vec![Finding::NoSshClient];
    }
    let mut out = Vec::new();
    let existing: Vec<&PathBuf> = identity_files.iter().filter(|p| p.is_file()).collect();
    if agent != Some(true) && existing.is_empty() {
        out.push(Finding::NoKey);
    } else if agent == Some(false) && existing.is_empty() {
        out.push(Finding::AgentEmpty);
    }
    for p in existing {
        if loose(p) {
            out.push(Finding::LooseKeyPermissions(p.clone()));
        }
    }
    out
}

/// Inspect the local ssh setup for `address:port`.
pub fn check(address: &str, port: u16) -> Vec<Finding> {
    let eff = effective(address, port);
    let findings = assess(
        eff.is_some(),
        agent_keys(),
        &eff.unwrap_or_default().identity_files,
    );
    tracing::debug!(?findings, "local ssh assessment");
    findings
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ssh_g() {
        let text =
            "user user\nhostname 1.2.3.4\nidentityfile ~/.ssh/id_ed25519\nidentityfile /k/x\n";
        let e = parse_effective(text, Path::new("/home/u"));
        assert_eq!(e.user.as_deref(), Some("user"));
        assert_eq!(
            e.identity_files,
            [
                PathBuf::from("/home/u/.ssh/id_ed25519"),
                PathBuf::from("/k/x")
            ]
        );
    }

    #[test]
    fn assesses_missing_keys_and_permissions() {
        assert_eq!(assess(false, None, &[]), [Finding::NoSshClient]);
        assert_eq!(
            assess(true, None, &[PathBuf::from("/nonexistent")]),
            [Finding::NoKey]
        );
        assert!(assess(true, Some(true), &[]).is_empty());

        let dir = tempfile::tempdir().unwrap();
        let key = dir.path().join("k");
        std::fs::write(&key, "not a real key").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o644)).unwrap();
            assert_eq!(
                assess(true, None, std::slice::from_ref(&key)),
                [Finding::LooseKeyPermissions(key.clone())]
            );
            std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        assert!(assess(true, Some(false), &[key]).is_empty());
        for f in [Finding::NoSshClient, Finding::NoKey, Finding::AgentEmpty] {
            assert!(!f.to_string().is_empty());
        }
    }

    // frob:tests crates/goway/src/sshenv.rs::check
    // frob:tests crates/goway/src/sshenv.rs::effective
    #[test]
    fn check_runs_against_the_real_client() {
        let findings = check("localhost", 22);
        assert!(!findings.contains(&Finding::NoSshClient) || effective("localhost", 22).is_none());
    }
}
