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

/// Pinned releases goway installs, with their sha256 per architecture.
struct Pinned {
    url: &'static str,
    sha256: &'static str,
    /// The member to extract (sccache's tarball nests it in a directory).
    member: &'static str,
    strip: u8,
}

const NEXTEST: [(&str, Pinned); 2] = [
    (
        "x86_64",
        Pinned {
            url: "https://github.com/nextest-rs/nextest/releases/download/cargo-nextest-0.9.146/cargo-nextest-0.9.146-x86_64-unknown-linux-gnu.tar.gz",
            sha256: "682c21b777c333e96fd532e114d3a5a894e0729ab88d94c0a9f20f8419695428",
            member: "cargo-nextest",
            strip: 0,
        },
    ),
    (
        "aarch64",
        Pinned {
            url: "https://github.com/nextest-rs/nextest/releases/download/cargo-nextest-0.9.146/cargo-nextest-0.9.146-aarch64-unknown-linux-gnu.tar.gz",
            sha256: "b2e33d7c72de7ade0ff7b3a948ac37516b24f8a836b7a8870c1f634a94be9de9",
            member: "cargo-nextest",
            strip: 0,
        },
    ),
];

const SCCACHE: [(&str, Pinned); 2] = [
    (
        "x86_64",
        Pinned {
            url: "https://github.com/mozilla/sccache/releases/download/v0.18.0/sccache-v0.18.0-x86_64-unknown-linux-musl.tar.gz",
            sha256: "45f1447fbe231e3037bde351ef70677dd212216c8d62ae7ca409fecc4d6acc89",
            member: "sccache-v0.18.0-x86_64-unknown-linux-musl/sccache",
            strip: 1,
        },
    ),
    (
        "aarch64",
        Pinned {
            url: "https://github.com/mozilla/sccache/releases/download/v0.18.0/sccache-v0.18.0-aarch64-unknown-linux-musl.tar.gz",
            sha256: "2b3284d5da3b46a47dc4229e75bb7b88ac4aa99c8d754fb7d2f84997e5a4354a",
            member: "sccache-v0.18.0-aarch64-unknown-linux-musl/sccache",
            strip: 1,
        },
    ),
];

/// The install command for a pinned release on `arch`: download, verify
/// the sha256, then extract into the user's cargo bin. `None` for an
/// architecture goway has no pinned build for (the host's report of its
/// own arch is never pasted into a command).
fn pinned_install(table: &[(&str, Pinned)], arch: &str) -> Option<String> {
    let (_, p) = table.iter().find(|(a, _)| *a == arch)?;
    Some(format!(
        "t=$(mktemp -d) && curl -fsSL {url} -o \"$t/a.tgz\" && echo \"{sha}  $t/a.tgz\" | sha256sum -c --quiet && mkdir -p {bin} && tar xzf \"$t/a.tgz\" -C {bin} --strip-components={strip} {member}; rc=$?; rm -rf \"$t\"; exit $rc",
        url = p.url,
        sha = p.sha256,
        bin = CARGO_BIN,
        strip = p.strip,
        member = p.member,
    ))
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
    match present("cargo-nextest") {
        Some(v) => push("cargo-nextest", Level::Ok, v, None),
        None => push(
            "cargo-nextest",
            Level::Warn,
            "missing; `cargo nextest run` will not work".to_owned(),
            pinned_install(&NEXTEST, arch).map(|command| Fix {
                command,
                root: false,
                why: "installs the pinned, checksum-verified prebuilt binary into the user's cargo bin".to_owned(),
            }),
        ),
    }
    match present("sccache") {
        Some(v) => push("sccache", Level::Ok, v, None),
        None => push(
            "sccache",
            Level::Warn,
            "missing; cold builds in new target slots will be slower".to_owned(),
            pinned_install(&SCCACHE, arch).map(|command| Fix {
                command,
                root: false,
                why: "installs the pinned, checksum-verified release binary into the user's cargo bin".to_owned(),
            }),
        ),
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

/// Run the fixes `--fix` allows: user fixes always; root fixes only with
/// `sudo` and only after `confirm` approves the whole list, and then all
/// together in ONE sudo session (one password, typed into sudo itself).
/// Each distinct command runs once.
pub fn apply_fixes(
    checks: &[Check],
    sudo: bool,
    confirm: &dyn Fn(&[Fix]) -> bool,
    runner: &dyn FixRunner,
) -> Applied {
    let mut applied = Applied::default();
    let mut seen = std::collections::BTreeSet::new();
    let mut root = Vec::new();
    let mut user = Vec::new();
    for fix in checks
        .iter()
        .filter(|c| c.level != Level::Ok)
        .filter_map(|c| c.fix.as_ref())
    {
        if seen.insert(fix.command.clone()) {
            if fix.root {
                root.push(fix.clone());
            } else {
                user.push(fix.clone());
            }
        }
    }
    // Root fixes first: they provide what user fixes need (curl, cc).
    if !root.is_empty() {
        if sudo && confirm(&root) {
            let script = format!(
                "set -e\n{}",
                root.iter()
                    .map(|f| f.command.as_str())
                    .collect::<Vec<_>>()
                    .join("\n")
            );
            tracing::info!(fixes = root.len(), "running root fixes in one sudo session");
            let ok = runner.run(&script, true);
            for f in &root {
                if ok {
                    applied.done.push(f.command.clone());
                } else {
                    applied.failed.push(f.command.clone());
                }
            }
        } else {
            applied.need_sudo = root;
        }
    }
    for fix in user {
        tracing::info!(command = %fix.command, "running fix");
        if runner.run(&fix.command, false) {
            applied.done.push(fix.command.clone());
        } else {
            applied.failed.push(fix.command.clone());
        }
    }
    applied
}

/// Real fix runner: ssh to the found host, `sudo` with a tty when needed.
pub(crate) struct SshFixRunner<'a> {
    pub(crate) found: &'a Found,
    pub(crate) settings: &'a ssh::Settings,
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

/// Something `doctor --fix` installed on a host. Only the name of the check
/// is stored: the undo command is derived from it when needed, never read
/// from the record, so a tampered record cannot make goway run its text.
/// (Older records also carried `undo` and `root`; they are ignored.)
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Installed {
    /// The check it fixed (`cargo`, `cargo-nextest`, ...).
    pub check: String,
    /// For `cargo`: whether `~/.cargo` existed before goway installed
    /// rustup. `None` (an older record) counts as "existed".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cargo_home_existed: Option<bool>,
}

/// What uninstall does about one recorded install.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Undo {
    /// Run this command (as root when `root`).
    Run {
        /// The command text, from [`undo_of`].
        command: String,
        /// Whether it needs administrator rights.
        root: bool,
    },
    /// A system package: listed, not removed.
    KeepPackage,
    /// rustup was set up on top of a `~/.cargo` that existed before.
    KeepCargo,
    /// Not a check goway knows; ignored.
    Unknown,
}

/// System-package checks: their installs are listed, never undone.
const PACKAGE_CHECKS: &[&str] = &["bash", "tar", "flock", "setsid", "curl", "cc (linker)"];

impl Installed {
    /// The action that takes this install back, derived from the check name.
    pub fn undo(&self) -> Undo {
        if self.check == "cargo" && self.cargo_home_existed != Some(false) {
            return Undo::KeepCargo;
        }
        if let Some((command, root)) = undo_of(&self.check) {
            return Undo::Run { command, root };
        }
        if PACKAGE_CHECKS.contains(&self.check.as_str()) {
            Undo::KeepPackage
        } else {
            Undo::Unknown
        }
    }
}

/// How to take back the fix of `check`; `None` for system packages, which
/// other software may have come to rely on (they are listed instead).
pub fn undo_of(check: &str) -> Option<(String, bool)> {
    match check {
        "cargo" => Some((format!("{CARGO_BIN}/rustup self uninstall -y"), false)),
        // Also the cargo bin directories the install created, if now empty.
        "cargo-nextest" => Some((
            format!(
                "rm -f {CARGO_BIN}/cargo-nextest && {{ rmdir {CARGO_BIN} 2>/dev/null && rmdir \"${{CARGO_HOME:-$HOME/.cargo}}\" 2>/dev/null; true; }}"
            ),
            false,
        )),
        "sccache" => Some((
            format!(
                "rm -f {CARGO_BIN}/sccache && {{ rmdir {CARGO_BIN} 2>/dev/null && rmdir \"${{CARGO_HOME:-$HOME/.cargo}}\" 2>/dev/null; true; }}"
            ),
            false,
        )),
        "sshd password login" => Some((
            "rm -f /etc/ssh/sshd_config.d/10-goway-keys-only.conf && systemctl reload ssh"
                .to_owned(),
            true,
        )),
        _ => None,
    }
}

/// Where the record of what goway installed on `host` lives.
pub fn installed_path(paths: &Paths, host: &str) -> std::path::PathBuf {
    paths
        .config_dir
        .join(format!("installed-{}.json", host.to_ascii_lowercase()))
}

/// Load the record of what goway installed on `host` (empty if none).
pub fn load_installed(paths: &Paths, host: &str) -> Vec<Installed> {
    std::fs::read_to_string(installed_path(paths, host))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

/// Add the checks whose fixes just ran to the host's record.
fn record_installed(
    paths: &Paths,
    host: &str,
    facts: &BTreeMap<String, String>,
    checks: &[Check],
    done: &[String],
) -> Result<()> {
    let mut items = load_installed(paths, host);
    let before = items.len();
    for c in checks {
        let ran = c.fix.as_ref().is_some_and(|f| done.contains(&f.command));
        if ran && !items.iter().any(|i| i.check == c.name) {
            items.push(Installed {
                check: c.name.clone(),
                cargo_home_existed: (c.name == "cargo")
                    .then(|| facts.get("cargo_home").is_none_or(|v| v != "0")),
            });
        }
    }
    if items.len() == before {
        return Ok(());
    }
    let text = serde_json::to_string_pretty(&items).map_err(|e| Error::Usage(e.to_string()))?;
    crate::config::write_atomic(&installed_path(paths, host), text.as_bytes())?;
    tracing::info!(host, items = items.len(), "recorded what doctor installed");
    Ok(())
}

/// List the root fixes with their reasons and ask once (or accept with --yes).
pub(crate) fn confirm_root(renderer: Renderer, host: &str, fixes: &[Fix], yes: bool) -> bool {
    renderer.headline(format_args!(
        "{} change(s) on {host} need administrator rights:",
        fixes.len()
    ));
    for f in fixes {
        renderer.line(format_args!("  {}\n    why: {}", f.display(), f.why));
    }
    if yes {
        return true;
    }
    match crate::render::ask(&format!(
        "Run them on {host} now? sudo there asks for {host}'s password once [y/N]: "
    )) {
        Some(answer) => matches!(answer.trim(), "y" | "Y" | "yes" | "Yes" | "YES"),
        None => false,
    }
}

fn show_applied(renderer: Renderer, host: &HostConfig, applied: &Applied) {
    for c in &applied.done {
        renderer.ok(format_args!("{}: fixed: {c}", host.name));
    }
    for c in &applied.failed {
        renderer.warn(format_args!("{}: fix failed: {c}", host.name));
    }
    if !applied.need_sudo.is_empty() {
        renderer.warn(format_args!(
            "{}: {} fix(es) need root and were not run. Rerun `goway doctor {} --fix --rsudo` to run them (sudo on {} asks for its password once; goway never sees it), or run them yourself:",
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
    if args.rsudo && !std::io::IsTerminal::is_terminal(&std::io::stdin()) {
        return Err(Error::Usage(
            "--rsudo needs an interactive terminal so the host's sudo can ask for the password"
                .to_owned(),
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
            let confirm = |fixes: &[Fix]| confirm_root(renderer, &host.name, fixes, args.yes);
            let applied = apply_fixes(&checks, args.rsudo, &confirm, &runner);
            show_applied(renderer, host, &applied);
            if let Err(e) = record_installed(paths, &host.name, &facts, &checks, &applied.done) {
                renderer.warn(format_args!(
                    "cannot record what was installed on {}: {e}",
                    host.name
                ));
            }
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
    fn an_unknown_arch_never_reaches_a_command() {
        let mut f = facts(&["cargo-nextest", "sccache"]);
        f.insert("arch".to_owned(), "x86_64; touch /tmp/pwned".to_owned());
        for c in assess(&f) {
            if let Some(fix) = &c.fix {
                assert!(!fix.command.contains("pwned"), "{}", fix.command);
            }
        }
    }

    #[test]
    fn tool_undo_removes_only_empty_directories_it_left() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let bin = home.join(".cargo/bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("sccache"), "x").unwrap();
        let (undo, root) = undo_of("sccache").unwrap();
        assert!(!root);
        let ok = std::process::Command::new("sh")
            .args(["-c", &undo])
            .env("HOME", home)
            .env_remove("CARGO_HOME")
            .status()
            .unwrap();
        assert!(ok.success());
        assert!(!home.join(".cargo").exists(), "empty dirs removed");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("cargo"), "x").unwrap();
        std::fs::write(bin.join("cargo-nextest"), "x").unwrap();
        let (undo, _) = undo_of("cargo-nextest").unwrap();
        let ok = std::process::Command::new("sh")
            .args(["-c", &undo])
            .env("HOME", home)
            .env_remove("CARGO_HOME")
            .status()
            .unwrap();
        assert!(ok.success());
        assert!(bin.join("cargo").exists(), "other tools stay");
    }

    #[test]
    fn records_never_supply_the_command_uninstall_runs() {
        // A hostile or legacy record: the stored undo and root are ignored.
        let items: Vec<Installed> = serde_json::from_str(
            r#"[{"check":"evil","undo":"touch /tmp/pwned","root":true},
                {"check":"sccache","undo":"touch /tmp/pwned","root":true},
                {"check":"tar","undo":"touch /tmp/pwned","root":false}]"#,
        )
        .unwrap();
        assert_eq!(items[0].undo(), Undo::Unknown);
        let (command, root) = undo_of("sccache").unwrap();
        assert_eq!(items[1].undo(), Undo::Run { command, root });
        assert_eq!(items[2].undo(), Undo::KeepPackage);
        let text = serde_json::to_string(&items[1]).unwrap();
        assert!(!text.contains("undo") && !text.contains("pwned"), "{text}");
    }

    #[test]
    fn cargo_undo_spares_a_cargo_home_that_existed_before() {
        let item = |existed| Installed {
            check: "cargo".to_owned(),
            cargo_home_existed: existed,
        };
        assert_eq!(item(Some(true)).undo(), Undo::KeepCargo);
        assert_eq!(item(None).undo(), Undo::KeepCargo, "old records are safe");
        assert!(matches!(item(Some(false)).undo(), Undo::Run { .. }));
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
        assert!(
            fix.command
                .contains("682c21b777c333e96fd532e114d3a5a894e0729ab88d94c0a9f20f8419695428")
        );
        assert!(fix.command.contains("sha256sum -c"), "{}", fix.command);
        assert!(!fix.command.contains("latest"), "{}", fix.command);
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
        let applied = apply_fixes(&checks, false, &|_| true, &rec);
        assert_eq!(applied.done.len(), 2, "nextest and sccache run as the user");
        assert!(rec.0.borrow().iter().all(|(_, sudo)| !sudo));
        assert_eq!(applied.need_sudo.len(), 1);
        assert!(applied.need_sudo[0].command.contains("build-essential"));
        assert!(applied.need_sudo[0].why.contains("root"));

        let rec = Recorder(RefCell::new(Vec::new()));
        let applied = apply_fixes(&checks, true, &|_| true, &rec);
        assert!(applied.need_sudo.is_empty());
        let calls = rec.0.borrow();
        assert!(calls[0].1, "root fixes first, under sudo");
        assert_eq!(calls.len(), 3);
    }

    #[test]
    fn root_fixes_need_confirmation_and_share_one_sudo_session() {
        let mut f = facts(&["cc", "curl"]);
        f.insert("password_auth".to_owned(), "default-yes".to_owned());
        let checks = assess(&f);
        let rec = Recorder(RefCell::new(Vec::new()));
        let declined = apply_fixes(&checks, true, &|_| false, &rec);
        assert!(
            rec.0.borrow().is_empty(),
            "nothing runs when the user says no"
        );
        assert_eq!(declined.need_sudo.len(), 3);
        let applied = apply_fixes(&checks, true, &|_| true, &rec);
        let calls = rec.0.borrow();
        let sudo_calls: Vec<&(String, bool)> = calls.iter().filter(|(_, s)| *s).collect();
        assert_eq!(sudo_calls.len(), 1, "one sudo session for all root fixes");
        assert!(sudo_calls[0].0.starts_with("set -e"));
        assert_eq!(applied.done.len(), 3);
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
                .is_some_and(|x| x.command.contains("aarch64-unknown-linux-gnu"))
        }));
    }
}
