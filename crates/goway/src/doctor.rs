//! `goway doctor [HOST] [--fix] [--sudo]`: check what a host needs and fix
//! what can be fixed.
//!
//! Checks come from one remote `doctor` call (facts as `key=value`) plus the
//! local ssh setup. Every problem names the exact command that fixes it.
//! `--fix` runs the fixes that need no root, as the ordinary user. Fixes
//! that need root are never run silently: goway prints each command with
//! its reason and asks the user to rerun with `--sudo`, which runs them
//! through an interactive ssh session so sudo can ask for the password.

use std::collections::BTreeMap;
use std::process::Stdio;

use crate::cli::DoctorArgs;
use crate::config::{Config, HostConfig};
use crate::error::{Error, Result};
use crate::paths::Paths;
use crate::pool;
use crate::remote;
use crate::render::Renderer;
use crate::resolve::{self, Found, Lookup, Prober};
use crate::ssh::{self, KeyPolicy};
use crate::sshenv;
use crate::state::State;

/// How bad a finding is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    /// Fine.
    Ok,
    /// Works, but slower or less safe than it should be.
    Warn,
    /// goway (or a Rust build) cannot work until fixed.
    Fail,
}

/// A command that fixes a finding, run on the host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fix {
    /// The shell command (without `sudo`).
    pub command: String,
    /// Whether it needs root.
    pub root: bool,
    /// Why it is needed, shown when asking for sudo.
    pub why: String,
}

impl Fix {
    /// The exact command line to type on the host (what goway runs).
    pub fn display(&self) -> String {
        if self.root {
            format!("sudo bash -c {}", ssh::shell_quote(&self.command))
        } else {
            self.command.clone()
        }
    }
}

/// One check result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    /// What was checked.
    pub name: String,
    /// The verdict.
    pub level: Level,
    /// Version found or what is wrong.
    pub detail: String,
    /// How to fix it, if goway knows.
    pub fix: Option<Fix>,
}

/// Parse `key=value` lines.
pub fn parse_facts(text: &str) -> BTreeMap<String, String> {
    text.lines()
        .filter_map(|l| l.split_once('='))
        .map(|(k, v)| (k.trim().to_owned(), v.trim().to_owned()))
        .collect()
}

fn tool<'a>(facts: &'a BTreeMap<String, String>, name: &str) -> Option<&'a str> {
    facts
        .get(&format!("tool.{name}"))
        .map(String::as_str)
        .filter(|v| !v.is_empty())
}

/// The package install command for this host's package manager.
fn install(facts: &BTreeMap<String, String>, apt: &str, dnf: &str, pacman: &str) -> String {
    if tool(facts, "apt-get").is_some() {
        format!("apt-get update && apt-get install -y {apt}")
    } else if tool(facts, "dnf").is_some() {
        format!("dnf install -y {dnf}")
    } else if tool(facts, "pacman").is_some() {
        format!("pacman -S --noconfirm {pacman}")
    } else {
        format!("install {apt} with the system package manager")
    }
}

const CARGO_BIN: &str = "\"${CARGO_HOME:-$HOME/.cargo}/bin\"";
const MIN_FREE: u64 = 10 * 1024 * 1024 * 1024;

/// Turn remote facts into checks with fixes; pure so it can be tested.
pub fn assess(facts: &BTreeMap<String, String>) -> Vec<Check> {
    let mut out = system_checks(facts);
    out.extend(toolchain_checks(facts));
    out.extend(host_checks(facts));
    out
}

/// Tools goway's remote side and the fixes need.
fn system_checks(facts: &BTreeMap<String, String>) -> Vec<Check> {
    let mut out = Vec::new();
    let mut push = |name: &str, level, detail: String, fix| {
        out.push(Check {
            name: name.to_owned(),
            level,
            detail,
            fix,
        });
    };
    let present = |name: &str| tool(facts, name).map(str::to_owned);
    for t in ["bash", "tar", "flock", "setsid"] {
        match present(t) {
            Some(v) => push(t, Level::Ok, v, None),
            None => push(
                t,
                Level::Fail,
                "missing; goway's remote side needs it".to_owned(),
                Some(Fix {
                    command: install(
                        facts,
                        "bash tar util-linux",
                        "bash tar util-linux",
                        "bash tar util-linux",
                    ),
                    root: true,
                    why: format!("goway runs its remote side with {t}; system packages need root"),
                }),
            ),
        }
    }
    let curl = present("curl");
    if curl.is_none() {
        push(
            "curl",
            Level::Warn,
            "missing; the toolchain fixes download with it".to_owned(),
            Some(Fix {
                command: install(facts, "curl ca-certificates", "curl", "curl"),
                root: true,
                why: "the rustup, nextest and sccache installers download with curl; system packages need root".to_owned(),
            }),
        );
    }
    match present("cc") {
        Some(v) => push("cc (linker)", Level::Ok, v, None),
        None => push(
            "cc (linker)",
            Level::Fail,
            "missing; cargo cannot link without a C toolchain".to_owned(),
            Some(Fix {
                command: install(facts, "build-essential", "gcc", "base-devel"),
                root: true,
                why: "cargo links through the system C compiler; system packages need root"
                    .to_owned(),
            }),
        ),
    }
    out
}

/// The Rust toolchain.
fn toolchain_checks(facts: &BTreeMap<String, String>) -> Vec<Check> {
    let mut out = Vec::new();
    let mut push = |name: &str, level, detail: String, fix| {
        out.push(Check {
            name: name.to_owned(),
            level,
            detail,
            fix,
        });
    };
    let present = |name: &str| tool(facts, name).map(str::to_owned);
    let arch = facts.get("arch").map_or("x86_64", String::as_str);
    match (present("rustup"), present("cargo")) {
        (_, Some(v)) => push("cargo", Level::Ok, v, None),
        (_, None) => push(
            "cargo",
            Level::Fail,
            "missing (no rustup toolchain for this user)".to_owned(),
            Some(Fix {
                command: "curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal".to_owned(),
                root: false,
                why: "rustup installs per user".to_owned(),
            }),
        ),
    }
    let nextest_url = if arch == "aarch64" {
        "https://get.nexte.st/latest/linux-arm"
    } else {
        "https://get.nexte.st/latest/linux"
    };
    match present("cargo-nextest") {
        Some(v) => push("cargo-nextest", Level::Ok, v, None),
        None => push(
            "cargo-nextest",
            Level::Warn,
            "missing; `cargo nextest run` will not work".to_owned(),
            Some(Fix {
                command: format!(
                    "mkdir -p {CARGO_BIN} && curl -LsSf {nextest_url} | tar zxf - -C {CARGO_BIN}"
                ),
                root: false,
                why: "installs the prebuilt binary into the user's cargo bin".to_owned(),
            }),
        ),
    }
    if let Some(v) = present("sccache") {
        push("sccache", Level::Ok, v, None);
    } else {
        {
            let triple = format!("{arch}-unknown-linux-musl");
            push(
                "sccache",
                Level::Warn,
                "missing; cold builds in new target slots will be slower".to_owned(),
                Some(Fix {
                    command: format!(
                        "v=$(curl -fsSL https://api.github.com/repos/mozilla/sccache/releases/latest | sed -n 's/.*\"tag_name\": *\"\\([^\"]*\\)\".*/\\1/p') && mkdir -p {CARGO_BIN} && curl -fsSL \"https://github.com/mozilla/sccache/releases/download/$v/sccache-$v-{triple}.tar.gz\" | tar xz --strip-components=1 -C {CARGO_BIN} \"sccache-$v-{triple}/sccache\""
                    ),
                    root: false,
                    why: "installs the prebuilt release binary into the user's cargo bin"
                        .to_owned(),
                }),
            );
        }
    }
    out
}

/// Disk and sshd hardening.
fn host_checks(facts: &BTreeMap<String, String>) -> Vec<Check> {
    let mut out = Vec::new();
    let mut push = |name: &str, level, detail: String, fix| {
        out.push(Check {
            name: name.to_owned(),
            level,
            detail,
            fix,
        });
    };
    match facts.get("disk_free").and_then(|v| v.parse::<u64>().ok()) {
        Some(free) if free < MIN_FREE => push(
            "disk",
            Level::Warn,
            format!(
                "{} free in the home directory",
                crate::status::human_bytes(free)
            ),
            None,
        ),
        Some(free) => push(
            "disk",
            Level::Ok,
            format!("{} free", crate::status::human_bytes(free)),
            None,
        ),
        None => {}
    }
    match facts.get("password_auth").map(String::as_str) {
        Some("no") => push("sshd password login", Level::Ok, "disabled (keys only)".to_owned(), None),
        Some(other) => push(
            "sshd password login",
            Level::Warn,
            format!("allowed ({other}); goway needs keys only"),
            Some(Fix {
                command: "printf 'PasswordAuthentication no\\nKbdInteractiveAuthentication no\\n' > /etc/ssh/sshd_config.d/10-goway-keys-only.conf && systemctl reload ssh".to_owned(),
                root: true,
                why: "password logins widen the attack surface and goway only uses keys; sshd config is owned by root (undo: remove /etc/ssh/sshd_config.d/10-goway-keys-only.conf)".to_owned(),
            }),
        ),
        None => {}
    }
    out
}

/// Runs fix commands on a host; injectable so the sudo policy is testable.
pub trait FixRunner {
    /// Run `command` as the user (`sudo == false`) or under sudo with a tty.
    fn run(&self, command: &str, sudo: bool) -> bool;
}

/// What `apply_fixes` did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Applied {
    /// Fixes that ran and succeeded.
    pub done: Vec<String>,
    /// Fixes that ran and failed.
    pub failed: Vec<String>,
    /// Root fixes not run because `--sudo` was not given.
    pub need_sudo: Vec<Fix>,
}

/// Run the fixes `--fix` allows: user fixes always, root fixes only with
/// `sudo`. Each distinct command runs once.
pub fn apply_fixes(checks: &[Check], sudo: bool, runner: &dyn FixRunner) -> Applied {
    let mut applied = Applied::default();
    let mut seen = std::collections::BTreeSet::new();
    // Root fixes first: they provide what user fixes need (curl, cc).
    let mut fixes: Vec<&Fix> = checks
        .iter()
        .filter(|c| c.level != Level::Ok)
        .filter_map(|c| c.fix.as_ref())
        .collect();
    fixes.sort_by_key(|f| !f.root);
    for fix in fixes {
        if !seen.insert(fix.command.clone()) {
            continue;
        }
        if fix.root && !sudo {
            applied.need_sudo.push(fix.clone());
            continue;
        }
        tracing::info!(command = %fix.command, root = fix.root, "running fix");
        if runner.run(&fix.command, fix.root) {
            applied.done.push(fix.command.clone());
        } else {
            applied.failed.push(fix.command.clone());
        }
    }
    applied
}

/// Real fix runner: ssh to the found host, `sudo` with a tty when needed.
struct SshFixRunner<'a> {
    found: &'a Found,
    settings: &'a ssh::Settings,
}

impl FixRunner for SshFixRunner<'_> {
    fn run(&self, command: &str, sudo: bool) -> bool {
        let script = if sudo {
            Fix {
                command: command.to_owned(),
                root: true,
                why: String::new(),
            }
            .display()
        } else {
            format!("bash -c {}", ssh::shell_quote(command))
        };
        let mut cmd = ssh::command(
            &self.found.target,
            self.settings,
            KeyPolicy::Strict,
            &script,
        );
        if sudo {
            ssh::force_tty(&mut cmd);
        }
        cmd.stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()
            .is_ok_and(|s| s.success())
    }
}

fn mark(level: Level) -> &'static str {
    match level {
        Level::Ok => "ok",
        Level::Warn => "WARN",
        Level::Fail => "FAIL",
    }
}

fn report(
    renderer: Renderer,
    host: &HostConfig,
    found: &Found,
    facts: &BTreeMap<String, String>,
    checks: &[Check],
) {
    renderer.headline(format_args!(
        "{} at {} ({}, {})",
        host.name,
        found.target.address,
        facts.get("os").map_or("unknown OS", String::as_str),
        facts.get("arch").map_or("?", String::as_str)
    ));
    let mut rows = vec![vec![
        "check".to_owned(),
        "status".to_owned(),
        "detail".to_owned(),
    ]];
    for c in checks {
        rows.push(vec![
            c.name.clone(),
            mark(c.level).to_owned(),
            c.detail.clone(),
        ]);
    }
    renderer.table(&rows);
    for c in checks.iter().filter(|c| c.level != Level::Ok) {
        if let Some(fix) = &c.fix {
            renderer.line(format_args!("  fix {}: {}", c.name, fix.display()));
        }
    }
}

/// Tell the user what `--fix` did and which root fixes still need sudo.
fn show_applied(renderer: Renderer, host: &HostConfig, applied: &Applied) {
    for c in &applied.done {
        renderer.ok(format_args!("{}: fixed: {c}", host.name));
    }
    for c in &applied.failed {
        renderer.warn(format_args!("{}: fix failed: {c}", host.name));
    }
    if !applied.need_sudo.is_empty() {
        renderer.warn(format_args!(
            "{}: {} fix(es) need root and were not run. Rerun `goway doctor {} --fix --sudo` to run them (sudo will ask for your password on {}), or run them yourself:",
            host.name,
            applied.need_sudo.len(),
            host.name,
            host.name
        ));
        for fix in &applied.need_sudo {
            renderer.line(format_args!("  {}\n    why: {}", fix.display(), fix.why));
        }
    }
}

/// `goway doctor`.
pub fn doctor(
    paths: &Paths,
    renderer: Renderer,
    args: &DoctorArgs,
    lookup: &(dyn Lookup + Sync),
    prober: &(dyn Prober + Sync),
    settings: &ssh::Settings,
) -> Result<u8> {
    if args.sudo && !std::io::IsTerminal::is_terminal(&std::io::stdin()) {
        return Err(Error::Usage(
            "--sudo needs an interactive terminal so sudo can ask for the password".to_owned(),
        ));
    }
    let config = Config::load(&paths.config_file())?;
    let hosts: Vec<&HostConfig> = match &args.host {
        Some(name) => vec![config.host(name)?],
        None => config.hosts.iter().collect(),
    };
    if hosts.is_empty() {
        renderer.note("no hosts configured; add one with `goway host add NAME`");
        return Ok(1);
    }
    let cmd = remote::invocation("doctor", &[config.defaults.remote_root.as_str()]);
    let mut state = State::load(&paths.state_file())?;
    let results = pool::on_hosts(&hosts, &mut state, |host, local| {
        resolve::resolve(
            &config,
            host,
            local,
            lookup,
            prober,
            KeyPolicy::Strict,
            &cmd,
        )
    });
    if let Err(e) = state.save(&paths.state_file()) {
        tracing::warn!(error = %e, "cannot cache host addresses");
    }
    let mut worst = Level::Ok;
    for (host, result) in results {
        let found = match result {
            Ok(found) => found,
            Err(e) => {
                renderer.warn(format_args!("{}: {e}", host.name));
                for finding in sshenv::check(
                    host.address.as_deref().unwrap_or(&host.name),
                    config.port_of(host),
                ) {
                    renderer.warn(format_args!("local ssh: {finding}"));
                }
                worst = Level::Fail;
                continue;
            }
        };
        let facts = parse_facts(&found.output);
        let mut checks = assess(&facts);
        for finding in sshenv::check(&found.target.address, found.target.port) {
            checks.push(Check {
                name: "local ssh".to_owned(),
                level: Level::Warn,
                detail: finding.to_string(),
                fix: None,
            });
        }
        report(renderer, host, &found, &facts, &checks);
        if args.fix {
            let runner = SshFixRunner {
                found: &found,
                settings,
            };
            let applied = apply_fixes(&checks, args.sudo, &runner);
            show_applied(renderer, host, &applied);
            if !applied.done.is_empty() || !applied.failed.is_empty() {
                // Re-check after fixing.
                let mut local = state.clone();
                let after = resolve::resolve(
                    &config,
                    host,
                    &mut local,
                    lookup,
                    prober,
                    KeyPolicy::Strict,
                    &cmd,
                )
                .map(|f| assess(&parse_facts(&f.output)));
                checks = after.unwrap_or(checks);
                renderer.note(format_args!("{}: after fixes:", host.name));
                report(renderer, host, &found, &facts, &checks);
            }
        }
        worst = worst.max(checks.iter().map(|c| c.level).max().unwrap_or(Level::Ok));
    }
    Ok(u8::from(worst == Level::Fail))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    fn facts(missing: &[&str]) -> BTreeMap<String, String> {
        let mut f = BTreeMap::new();
        for t in [
            "bash",
            "git",
            "tar",
            "flock",
            "setsid",
            "cc",
            "curl",
            "rustup",
            "cargo",
            "cargo-nextest",
            "sccache",
            "apt-get",
        ] {
            let v = if missing.contains(&t) {
                String::new()
            } else {
                format!("{t} 1.0")
            };
            f.insert(format!("tool.{t}"), v);
        }
        f.insert("arch".to_owned(), "x86_64".to_owned());
        f.insert("disk_free".to_owned(), (500u64 << 30).to_string());
        f.insert("password_auth".to_owned(), "no".to_owned());
        f
    }

    #[test]
    fn healthy_host_is_all_ok() {
        assert!(assess(&facts(&[])).iter().all(|c| c.level == Level::Ok));
    }

    #[test]
    fn missing_nextest_names_the_exact_fix() {
        let checks = assess(&facts(&["cargo-nextest"]));
        let c = checks.iter().find(|c| c.name == "cargo-nextest").unwrap();
        assert_eq!(c.level, Level::Warn);
        let fix = c.fix.as_ref().unwrap();
        assert!(!fix.root);
        assert_eq!(
            fix.command,
            "mkdir -p \"${CARGO_HOME:-$HOME/.cargo}/bin\" && curl -LsSf https://get.nexte.st/latest/linux | tar zxf - -C \"${CARGO_HOME:-$HOME/.cargo}/bin\""
        );
    }

    #[test]
    fn missing_linker_and_password_login_need_root() {
        let mut f = facts(&["cc"]);
        f.insert("password_auth".to_owned(), "default-yes".to_owned());
        let checks = assess(&f);
        let cc = checks.iter().find(|c| c.name == "cc (linker)").unwrap();
        assert_eq!(cc.level, Level::Fail);
        let fix = cc.fix.as_ref().unwrap();
        assert!(fix.root && fix.command.contains("apt-get install -y build-essential"));
        assert!(fix.why.contains("root"));
        assert_eq!(
            fix.display(),
            "sudo bash -c 'apt-get update && apt-get install -y build-essential'"
        );
        let pw = checks
            .iter()
            .find(|c| c.name == "sshd password login")
            .unwrap();
        assert!(pw.fix.as_ref().unwrap().root);
    }

    struct Recorder(RefCell<Vec<(String, bool)>>);
    impl FixRunner for Recorder {
        fn run(&self, command: &str, sudo: bool) -> bool {
            self.0.borrow_mut().push((command.to_owned(), sudo));
            true
        }
    }

    #[test]
    fn root_fixes_never_run_without_sudo() {
        let checks = assess(&facts(&["cc", "cargo-nextest", "sccache"]));
        let rec = Recorder(RefCell::new(Vec::new()));
        let applied = apply_fixes(&checks, false, &rec);
        assert_eq!(applied.done.len(), 2, "nextest and sccache run as the user");
        assert!(rec.0.borrow().iter().all(|(_, sudo)| !sudo));
        assert_eq!(applied.need_sudo.len(), 1);
        assert!(applied.need_sudo[0].command.contains("build-essential"));
        assert!(applied.need_sudo[0].why.contains("root"));

        let rec = Recorder(RefCell::new(Vec::new()));
        let applied = apply_fixes(&checks, true, &rec);
        assert!(applied.need_sudo.is_empty());
        let calls = rec.0.borrow();
        assert!(calls[0].1, "root fixes first, under sudo");
        assert_eq!(calls.len(), 3);
    }

    #[test]
    fn other_package_managers_and_arm() {
        let mut f = facts(&["cc", "apt-get", "cargo-nextest"]);
        f.insert("tool.dnf".to_owned(), "dnf 4".to_owned());
        f.insert("arch".to_owned(), "aarch64".to_owned());
        let checks = assess(&f);
        assert!(checks.iter().any(|c| {
            c.fix
                .as_ref()
                .is_some_and(|x| x.command == "dnf install -y gcc")
        }));
        assert!(checks.iter().any(|c| {
            c.fix
                .as_ref()
                .is_some_and(|x| x.command.contains("linux-arm"))
        }));
    }
}
