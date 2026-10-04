//! What `goway doctor` prints, as pure functions over the checks so the
//! wording can be pinned by golden tests: one summary line per host, then
//! each problem once (grouped across the hosts that share it), the passing
//! rows only on request, long explanations only for one named check, the
//! fixes listed once in plain words, and hardening kept apart as optional.

use std::fmt::Write as _;

use super::{Check, Fix, HARDENING, Level};
use crate::render;

/// What happened when doctor looked at one host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The host could not be reached; the text says why.
    Down(String),
    /// The checks that ran there.
    Checked(Vec<Check>),
}

/// One host's result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostReport {
    /// The configured name.
    pub name: String,
    /// The address it was reached at.
    pub address: String,
    /// The operating system it reports.
    pub os: String,
    /// Its CPU architecture.
    pub arch: String,
    /// What was found.
    pub outcome: Outcome,
}

impl HostReport {
    fn checks(&self) -> &[Check] {
        match &self.outcome {
            Outcome::Checked(c) => c,
            Outcome::Down(_) => &[],
        }
    }
}

/// Whether a check is optional hardening rather than something goway needs.
pub fn is_hardening(check: &Check) -> bool {
    HARDENING.contains(&check.name.as_str())
}

/// One row of the problem table: a finding and the hosts that share it.
struct Group {
    level: Level,
    name: String,
    detail: String,
    hosts: Vec<String>,
}

fn word(level: Level) -> &'static str {
    match level {
        Level::Ok => "ok",
        Level::Warn => "WARN",
        Level::Fail => "FAIL",
    }
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// The hosts, in words: "all 3 hosts" when every checked host shares it.
fn hosts_text(hosts: &[String], checked: usize) -> String {
    if hosts.len() > 1 && hosts.len() == checked {
        format!("all {checked} hosts")
    } else {
        hosts.join(", ")
    }
}

/// Group the findings of the hosts: the same check with the same verdict and
/// detail is one group. Problems first (FAIL before WARN), then by name.
fn groups(hosts: &[HostReport], want: impl Fn(&Check) -> bool) -> Vec<Group> {
    let mut out: Vec<Group> = Vec::new();
    for host in hosts {
        for c in host.checks().iter().filter(|c| want(c)) {
            match out
                .iter_mut()
                .find(|g| g.name == c.name && g.level == c.level && g.detail == c.detail)
            {
                Some(g) => g.hosts.push(host.name.clone()),
                None => out.push(Group {
                    level: c.level,
                    name: c.name.clone(),
                    detail: c.detail.clone(),
                    hosts: vec![host.name.clone()],
                }),
            }
        }
    }
    out.sort_by(|a, b| b.level.cmp(&a.level).then_with(|| a.name.cmp(&b.name)));
    out
}

fn table(rows: &[Vec<String>], plain: bool) -> Vec<String> {
    if plain {
        render::plain_table(rows)
    } else {
        render::format_table(rows)
    }
}

/// The report: a summary line per host, then the problems (and, with `all`,
/// the passing checks too) in one table, then what to do next.
pub fn report_lines(hosts: &[HostReport], all: bool, plain: bool) -> Vec<String> {
    let mut lines = Vec::new();
    let checked = hosts
        .iter()
        .filter(|h| matches!(h.outcome, Outcome::Checked(_)))
        .count();
    for h in hosts {
        match &h.outcome {
            Outcome::Down(why) => lines.push(format!("{}: unreachable: {why}", h.name)),
            Outcome::Checked(checks) => {
                let bad = checks.iter().filter(|c| c.level != Level::Ok).count();
                let ok = checks.len() - bad;
                let verdict = if bad == 0 {
                    format!("all {ok} ok")
                } else {
                    format!("{}, {ok} ok", plural(bad, "problem", "problems"))
                };
                lines.push(format!(
                    "{} ({}, {}, {}): {verdict}",
                    h.name, h.address, h.os, h.arch
                ));
            }
        }
    }
    let mut rows = vec![vec![
        "status".to_owned(),
        "check".to_owned(),
        "detail".to_owned(),
        "hosts".to_owned(),
    ]];
    for g in groups(hosts, |c| c.level != Level::Ok)
        .into_iter()
        .chain(if all {
            groups(hosts, |c| c.level == Level::Ok)
        } else {
            Vec::new()
        })
    {
        rows.push(vec![
            word(g.level).to_owned(),
            g.name,
            g.detail,
            hosts_text(&g.hosts, checked),
        ]);
    }
    if rows.len() > 1 {
        lines.push(String::new());
        lines.extend(table(&rows, plain));
    }
    let problems: usize = hosts
        .iter()
        .flat_map(HostReport::checks)
        .filter(|c| c.level != Level::Ok)
        .count();
    if problems > 0 {
        lines.push(String::new());
        lines.push(
            "goway doctor --fix applies the fixes; goway doctor --explain CHECK says more about one check."
                .to_owned(),
        );
    }
    lines
}

/// The long text for one check: what each host found, why, and the exact
/// command of its fix. Names the known checks when `check` matches none.
pub fn explain_lines(hosts: &[HostReport], check: &str) -> Vec<String> {
    let mut lines = Vec::new();
    let mut known: Vec<&str> = Vec::new();
    let mut shown: Vec<(String, Level, String)> = Vec::new();
    for h in hosts {
        for c in h.checks() {
            if !known.contains(&c.name.as_str()) {
                known.push(&c.name);
            }
            if c.name != check {
                continue;
            }
            let key = (c.name.clone(), c.level, c.detail.clone());
            let first = !shown.contains(&key);
            if first {
                shown.push(key);
            }
            let on: Vec<String> = hosts
                .iter()
                .filter(|o| {
                    o.checks()
                        .iter()
                        .any(|x| x.name == c.name && x.level == c.level && x.detail == c.detail)
                })
                .map(|o| o.name.clone())
                .collect();
            if !first {
                continue;
            }
            lines.push(format!(
                "{} {}: {}  [{}]",
                word(c.level),
                c.name,
                c.detail,
                on.join(", ")
            ));
            if let Some(text) = &c.explain {
                lines.push(format!("  {text}"));
            }
            if let Some(fix) = &c.fix {
                lines.push(format!("  fix: {}", describe(c)));
                lines.push(format!("  why: {}", fix.why));
                lines.push(format!("  exact: {}", fix.display()));
            }
        }
    }
    if lines.is_empty() {
        lines.push(format!(
            "no check named `{check}`; the checks are: {}",
            known.join(", ")
        ));
    }
    lines
}

/// A fix in plain words (never the command).
pub fn describe(check: &Check) -> String {
    match &check.fix {
        Some(fix) => describe_fix(&check.name, fix),
        None => "nothing goway can do".to_owned(),
    }
}

/// The fix `fix` of the check `name`, in plain words.
pub fn describe_fix(name: &str, fix: &Fix) -> String {
    if let Some(words) = package_words(fix) {
        let by = if fix.why.starts_with("from repository") {
            format!(", {}", fix.why)
        } else {
            String::new()
        };
        return format!(
            "install {words} with the system package manager (administrator rights){by}"
        );
    }
    match name {
        "sshd password login" => "turn off ssh password login (keys only)".to_owned(),
        "cuda toolkit" => "install NVIDIA's CUDA toolkit (administrator rights)".to_owned(),
        name if fix.root => format!("change {name} (administrator rights)"),
        name => format!("set up {name} for your user (pinned, checksum-verified downloads)"),
    }
}

/// The package names a system-package fix installs.
fn package_words(fix: &Fix) -> Option<String> {
    if !fix.root {
        return None;
    }
    let cmd = fix.command.rsplit("&&").next()?.trim();
    cmd.strip_prefix("apt-get install -y ")
        .or_else(|| cmd.strip_prefix("dnf install -y "))
        .or_else(|| cmd.strip_prefix("pacman -S --noconfirm "))
        .map(str::to_owned)
}

/// The fixes, each once, with the hosts that need it. Tool fixes first;
/// hardening is a separate, optional list, applied only with `--harden`.
pub fn plan_lines(hosts: &[HostReport], harden: bool, rsudo: bool) -> Vec<String> {
    let mut lines = Vec::new();
    let section = |lines: &mut Vec<String>, want: &dyn Fn(&Check) -> bool, title: &str| {
        let mut items: Vec<(String, String, Vec<String>)> = Vec::new();
        for h in hosts {
            for c in h
                .checks()
                .iter()
                .filter(|c| c.level != Level::Ok && c.fix.is_some() && want(c))
            {
                let text = describe(c);
                match items
                    .iter_mut()
                    .find(|(n, t, _)| *n == c.name && *t == text)
                {
                    Some((_, _, on)) => on.push(h.name.clone()),
                    None => items.push((c.name.clone(), text, vec![h.name.clone()])),
                }
            }
        }
        if items.is_empty() {
            return;
        }
        lines.push(title.to_owned());
        for (name, text, on) in items {
            lines.push(format!("  {name}: {text}  on {}", on.join(", ")));
        }
    };
    section(&mut lines, &|c| !is_hardening(c), "Fixes:");
    let title = if harden && rsudo {
        "Optional hardening (applied now, asked separately):"
    } else {
        "Optional hardening (not applied; rerun with --harden --rsudo):"
    };
    section(&mut lines, &is_hardening, title);
    lines
}

/// The question asked before root fixes run on `host`: what will change
/// there, in plain words, and how to see the exact commands.
pub fn prompt_text(host: &str, steps: &[(String, Fix)], hardening: bool) -> String {
    let mut text = format!(
        "On {host}, as administrator, goway will{}:\n",
        if hardening {
            " apply this hardening"
        } else {
            ""
        }
    );
    for (name, fix) in steps {
        let _ = writeln!(text, "  - {}", describe_fix(name, fix));
    }
    let _ = write!(
        text,
        "sudo on {host} asks for its password (goway never sees it). Go ahead? [s]how exact commands / [y]es / [N]o: "
    );
    text
}

/// The exact root script, indented, as shown before it runs.
pub fn script_lines(host: &str, script: &str) -> Vec<String> {
    let mut lines = vec![format!(
        "exact commands for {host}, run as root in one sudo session:"
    )];
    for l in script.lines() {
        let mut line = String::new();
        let _ = write!(line, "    {l}");
        lines.push(line);
    }
    lines
}
