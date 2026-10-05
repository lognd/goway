//! Diagnose a WSL helper whose ssh port does not answer by looking at it from the Windows side.
//!
//! When the WSL sshd cannot be reached, the same address may still answer Windows OpenSSH
//! (port 22). Through it doctor runs two read-only commands: `wsl.exe -l -v` (is the distro
//! stopped?) and `schtasks /query /v /fo list` (is the keepalive task there, and does it repeat?).
//! Nothing is started or changed: the fix is printed for the user to run. Both outputs are
//! parsed by pure functions so the verdicts are testable without a machine.

use std::fmt::Write as _;

use super::{Check, Level};
use crate::resolve::Prober;
use crate::ssh::{Failure, KeyPolicy, Target};
use crate::transport;

/// The port Windows OpenSSH listens on.
pub const WINDOWS_SSH_PORT: u16 = 22;

/// The keepalive task's name for the default profile; goway-setup registers it (`host::task_name`),
/// with `(boot)` appended for a boot keepalive and a profile prefix for a named profile.
pub const KEEPALIVE_TASK: &str = "WSL Keepalive";

/// One distro as `wsl.exe -l -v` lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Distro {
    /// The distro name.
    pub name: String,
    /// `Running`, `Stopped`, ...
    pub state: String,
    /// Marked with `*`, the distro `wsl` starts by default.
    pub default: bool,
}

/// A keepalive scheduled task as `schtasks /query /v /fo list` reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Task {
    /// The task name without its folder.
    pub name: String,
    /// `Ready`, `Running`, `Disabled`, ...
    pub status: String,
    /// The last run's result code as printed.
    pub last_result: String,
    /// Whether any trigger of the task repeats.
    pub repeats: bool,
}

/// Windows text with its UTF-16 leftovers removed: `wsl.exe` prints UTF-16, which arrives as text
/// with a NUL after every character (and maybe a byte order mark).
pub fn clean_windows_text(raw: &str) -> String {
    raw.chars()
        .filter(|c| !matches!(c, '\0' | '\u{feff}' | '\u{fffd}' | '\r'))
        .collect()
}

/// The distros in `wsl.exe -l -v` output (already cleaned).
pub fn parse_wsl_list(text: &str) -> Vec<Distro> {
    text.lines()
        .filter_map(|line| {
            let default = line.trim_start().starts_with('*');
            let words: Vec<&str> = line
                .trim_start()
                .trim_start_matches('*')
                .split_whitespace()
                .collect();
            // NAME STATE VERSION; the header row has the word STATE in that place.
            let [name, state, version] = words[..] else {
                return None;
            };
            (state != "STATE" && version.chars().all(|c| c.is_ascii_digit())).then(|| Distro {
                name: name.to_owned(),
                state: state.to_owned(),
                default,
            })
        })
        .collect()
}

/// Whether `name` (folder stripped) is a keepalive task of any profile, logon or boot.
fn is_keepalive(name: &str) -> bool {
    let base = name.strip_suffix(" (boot)").unwrap_or(name);
    base == KEEPALIVE_TASK || base.ends_with(&format!(" {KEEPALIVE_TASK}"))
}

/// The keepalive tasks in `schtasks /query /v /fo list` output (already cleaned). A task with
/// several triggers is listed once per trigger; it repeats when any of them does.
pub fn parse_keepalive_tasks(text: &str) -> Vec<Task> {
    let mut tasks: Vec<Task> = Vec::new();
    let mut current: Option<usize> = None;
    for line in text.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        match key.trim() {
            "TaskName" => {
                let name = value.rsplit('\\').next().unwrap_or(value).to_owned();
                current = None;
                if is_keepalive(&name) {
                    let index = tasks
                        .iter()
                        .position(|t| t.name == name)
                        .unwrap_or_else(|| {
                            tasks.push(Task {
                                name,
                                status: String::new(),
                                last_result: String::new(),
                                repeats: false,
                            });
                            tasks.len() - 1
                        });
                    current = Some(index);
                }
            }
            "Status" => {
                if let Some(i) = current {
                    value.clone_into(&mut tasks[i].status);
                }
            }
            "Last Result" => {
                if let Some(i) = current {
                    value.clone_into(&mut tasks[i].last_result);
                }
            }
            // The key is `Repeat: Every`; split_once cut it at the first colon.
            "Repeat" => {
                if let Some(i) = current
                    && let Some(rest) = value.strip_prefix("Every:")
                {
                    let rest = rest.trim();
                    if !rest.is_empty() && rest != "Disabled" && rest != "N/A" {
                        tasks[i].repeats = true;
                    }
                }
            }
            _ => {}
        }
    }
    tasks
}

fn check(name: &str, level: Level, detail: String) -> Check {
    Check {
        name: name.to_owned(),
        level,
        detail,
        fix: None,
        explain: None,
    }
}

/// The command that starts the keepalive over Windows ssh, for the user to run.
fn start_command(windows: &Target, task: &str) -> String {
    let who = match &windows.user {
        Some(u) => format!("{u}@{}", windows.address),
        None => windows.address.clone(),
    };
    format!("ssh -p {} {who} schtasks /run /tn \"{task}\"", windows.port)
}

/// Run `script` on Windows through its OpenSSH and return its cleaned output.
fn windows_output(
    prober: &dyn Prober,
    windows: &Target,
    script: &str,
) -> Result<String, (Failure, String)> {
    prober
        .probe(
            windows,
            KeyPolicy::Strict,
            &transport::windows_ssh_line(script),
        )
        .map(|out| clean_windows_text(&out))
}

/// The findings for a helper whose WSL ssh port `wsl_port` did not answer, from asking Windows
/// OpenSSH at `windows` (the same address, port [`WINDOWS_SSH_PORT`]). Read-only.
pub fn diagnose(prober: &dyn Prober, windows: &Target, wsl_port: u16) -> Vec<Check> {
    tracing::info!(host = %windows.name, address = %windows.address, wsl_port, "asking Windows about an unreachable WSL helper");
    let list = match windows_output(prober, windows, "wsl.exe -l -v") {
        Ok(text) => text,
        Err((Failure::Unreachable, why)) => {
            tracing::warn!(host = %windows.name, %why, "nothing answers at the address");
            return vec![check(
                "machine",
                Level::Fail,
                format!(
                    "unreachable: nothing answers at {} on port {wsl_port} (WSL) or {} (Windows ssh); it is off, asleep, off the network or blocked, which is not the same as WSL being stopped",
                    windows.address, windows.port
                ),
            )];
        }
        Err((failure, why)) => {
            tracing::warn!(host = %windows.name, ?failure, "Windows ssh answers but refuses the login");
            let hint = match failure {
                Failure::HostKeyUnknown => format!(
                    "; its host key is not pinned: check `ssh -p {} {} wsl -l -v` by hand once, or add it with `goway host add`",
                    windows.port, windows.address
                ),
                _ => String::new(),
            };
            return vec![check(
                "windows ssh",
                Level::Warn,
                format!(
                    "the machine answers on port {} but goway cannot log in to Windows ({}){hint}; cannot tell whether WSL is stopped",
                    windows.port,
                    why.lines().next().unwrap_or("no detail")
                ),
            )];
        }
    };
    let tasks = match windows_output(prober, windows, "schtasks /query /v /fo list") {
        Ok(text) => parse_keepalive_tasks(&text),
        Err((_, why)) => {
            tracing::warn!(host = %windows.name, %why, "cannot list scheduled tasks");
            Vec::new()
        }
    };
    let task_name = tasks.first().map_or(KEEPALIVE_TASK, |t| t.name.as_str());
    let mut checks = Vec::new();
    let distros = parse_wsl_list(&list);
    let subject = distros.iter().find(|d| d.default).or(distros.first());
    checks.push(match subject {
        None => check(
            "wsl distro",
            Level::Warn,
            format!("Windows answers but `wsl -l -v` lists no distro; run `ssh -p {} {} wsl -l -v` by hand", windows.port, windows.address),
        ),
        Some(d) if d.state.eq_ignore_ascii_case("Stopped") => check(
            "wsl distro",
            Level::Fail,
            format!(
                "WSL distro {} is stopped (Windows answers, WSL ssh port {wsl_port} does not); start the keepalive: {}",
                d.name,
                start_command(windows, task_name)
            ),
        ),
        Some(d) => check(
            "wsl distro",
            Level::Warn,
            format!(
                "WSL distro {} is {}, yet its ssh port {wsl_port} does not answer; check sshd and the firewall rule inside and outside WSL",
                d.name,
                d.state.to_ascii_lowercase()
            ),
        ),
    });
    match tasks.first() {
        None => checks.push(check(
            "keepalive task",
            Level::Warn,
            "no WSL Keepalive scheduled task found; run goway-setup install on the host".to_owned(),
        )),
        Some(t) if !tasks.iter().any(|t| t.repeats) => checks.push(check(
            "keepalive task",
            Level::Warn,
            format!(
                "{} has only a boot or logon trigger (status {}, last result {}): a WSL shutdown leaves the helper offline until the next boot; re-run goway-setup install to replace it with the repeating task",
                t.name, t.status, t.last_result
            ),
        )),
        Some(t) => checks.push(check(
            "keepalive task",
            Level::Ok,
            format!("{} repeats (status {}, last result {})", t.name, t.status, t.last_result),
        )),
    }
    checks
}

/// The text of an unreachable host's report: why goway failed, then each finding.
pub fn describe(why: &str, checks: &[Check]) -> String {
    let mut text = why.to_owned();
    for c in checks {
        let _ = write!(text, "\n    {}: {}", c.name, c.detail);
    }
    text
}
