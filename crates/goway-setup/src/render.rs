//! The one module that prints; everything goway-setup itself says goes through [`Renderer`].
#![allow(clippy::print_stdout, clippy::print_stderr, clippy::disallowed_macros)]

use std::io::Write as _;

use anstream::{AutoStream, ColorChoice};
use anstyle::{AnsiColor, Style};
use goway_journal::{Change, Outcome, RegValue, ResourceKind};

use crate::app::{StatusRow, UninstallReport};
use crate::error::SetupError;
use crate::layout::Layout;

const ERROR: Style = AnsiColor::Red.on_default().bold();
const WARN: Style = AnsiColor::Yellow.on_default().bold();
const GOOD: Style = AnsiColor::Green.on_default().bold();
const DIM: Style = Style::new().dimmed();

/// When to color output, as chosen with `--color`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, clap::ValueEnum)]
pub enum ColorWhen {
    /// Color when the stream is a terminal and `NO_COLOR` is unset.
    #[default]
    Auto,
    /// Always color.
    Always,
    /// Never color.
    Never,
}

/// One-line, human description of a change (pure; used by the plan, status and tests).
pub fn describe(change: &Change) -> String {
    match change {
        Change::WriteFile { path, .. } => format!("write file {}", path.display()),
        Change::EnsureLine { path, line, .. } => {
            format!("ensure line {line:?} in {}", path.display())
        }
        Change::EnsureDir { path } => format!("ensure directory {}", path.display()),
        Change::EnsureListEntry { var, entry, .. } => format!("add {entry} to user {var}"),
        Change::InstallFile { path, digest, .. } => {
            format!(
                "install file {} (sha256 {})",
                path.display(),
                &digest[..digest.len().min(12)]
            )
        }
        Change::EnsureRegKey { key } => format!("ensure registry key {key}"),
        Change::SetRegistryValue { key, name, value } => {
            format!("set registry value {key}\\{name} = {}", show_value(value))
        }
        Change::SetIniKey {
            path,
            section,
            key,
            value,
        } => {
            format!("set [{section}] {key}={value} in {}", path.display())
        }
        Change::SetUnixMode { path, mode } => format!("chmod {mode:o} {}", path.display()),
        Change::SetAcl { path, .. } => format!("set ACL of {}", path.display()),
        Change::EnsureResource { kind, name, .. } => {
            format!("ensure {} {name}", describe_kind(*kind))
        }
    }
}

/// Human name of a resource kind.
pub fn describe_kind(kind: ResourceKind) -> &'static str {
    match kind {
        ResourceKind::FirewallRule => "Windows Firewall rule",
        ResourceKind::HyperVFirewallRule => "Hyper-V firewall rule",
        ResourceKind::ScheduledTask => "scheduled task",
        ResourceKind::PortProxy => "Windows port relay (portproxy)",
        ResourceKind::Service => "service",
        ResourceKind::WslPackage => "WSL package",
        ResourceKind::WslUnit => "enabled WSL systemd unit",
        ResourceKind::SshKeyPair => "ssh key pair",
    }
}

fn show_value(value: &RegValue) -> String {
    match value {
        RegValue::String(s) => format!("{s:?}"),
        RegValue::ExpandString(s) => format!("{s:?} (expandable)"),
        RegValue::Dword(d) => d.to_string(),
    }
}

/// Prints goway-setup's own messages with consistent styling.
#[derive(Debug, Clone, Copy)]
pub struct Renderer {
    choice: ColorChoice,
}

impl Renderer {
    /// Build a renderer honouring `--color` (and `NO_COLOR` under `auto`).
    pub fn new(when: ColorWhen) -> Self {
        let choice = match when {
            ColorWhen::Auto => ColorChoice::Auto,
            ColorWhen::Always => ColorChoice::Always,
            ColorWhen::Never => ColorChoice::Never,
        };
        Self { choice }
    }

    fn out(self) -> AutoStream<std::io::Stdout> {
        AutoStream::new(std::io::stdout(), self.choice)
    }

    fn err(self) -> AutoStream<std::io::Stderr> {
        AutoStream::new(std::io::stderr(), self.choice)
    }

    fn line(self, style: Style, tag: &str, text: &str) {
        let _ = writeln!(self.out(), "{style}{tag:>10}{style:#} {text}");
    }

    /// Report a failure on stderr.
    pub fn error(self, error: &SetupError) {
        let _ = writeln!(self.err(), "{ERROR}error{ERROR:#}: {error}");
    }

    /// Print the plan of a dry run.
    pub fn plan(self, layout: &Layout, plan: &[Change]) {
        self.line(
            WARN,
            "dry run",
            &format!(
                "profile {} would apply {} changes:",
                layout.profile,
                plan.len()
            ),
        );
        for (i, c) in plan.iter().enumerate() {
            let _ = writeln!(self.out(), "  {DIM}{i:>2}{DIM:#} {}", describe(c));
        }
    }

    /// Announce a finished install.
    pub fn installed(self, layout: &Layout, applied: usize) {
        self.line(
            GOOD,
            "installed",
            &format!("profile {} ({applied} changes)", layout.profile),
        );
        self.line(DIM, "location", &layout.install_root.display().to_string());
        self.line(DIM, "journal", &layout.journal_path.display().to_string());
        self.line(
            DIM,
            "note",
            "open a new terminal to pick up the Path change",
        );
    }

    /// Announce that the uninstaller handed over to a temporary copy.
    pub fn relaunching(self, copy: &std::path::Path, log: &std::path::Path) {
        self.line(
            WARN,
            "handover",
            &format!(
                "continuing in the background from {} so this file can be deleted",
                copy.display()
            ),
        );
        self.line(DIM, "log", &log.display().to_string());
    }

    /// Announce that there was nothing to uninstall.
    pub fn nothing_installed(self, layout: &Layout) {
        self.line(
            WARN,
            "skipped",
            &format!("profile {} has no install journal", layout.profile),
        );
    }

    /// Print the outcome of an uninstall of the whole profile.
    pub fn uninstalled(self, layout: &Layout, report: &UninstallReport) {
        self.uninstalled_component(layout, "profile", report);
    }

    /// Print the outcome of an uninstall of one component (`label` names what was removed).
    pub fn uninstalled_component(self, layout: &Layout, label: &str, report: &UninstallReport) {
        for (index, outcome) in &report.outcomes {
            let change = &report.journal.entries[*index].change;
            let (style, tag, extra) = match outcome {
                Outcome::Restored => (GOOD, "restored", String::new()),
                Outcome::Noop => (DIM, "unchanged", " (was already in place)".to_owned()),
                Outcome::AlreadyReverted => (DIM, "done", String::new()),
                Outcome::LeftAlone(why) => (WARN, "kept", format!(" ({why})")),
            };
            self.line(style, tag, &format!("{}{extra}", describe(change)));
        }
        self.line(GOOD, "removed", &format!("{label} {}", layout.profile));
    }

    /// Print text produced by another process (an elevated run's captured output) unchanged.
    pub fn passthrough(self, text: &str) {
        let _ = write!(self.out(), "{text}");
    }

    /// Announce that the installer asks Windows for administrator rights.
    pub fn elevating(self, what: &str) {
        self.line(
            WARN,
            "elevating",
            &format!("{what} needs administrator rights; accept the Windows prompt"),
        );
    }

    /// Print a multi-line block of plain text (the next-steps hand-over) after a blank line.
    pub fn block(self, text: &str) {
        let _ = writeln!(self.out(), "\n{text}");
    }

    /// Print a question without a trailing newline so the answer follows it on the same line.
    pub fn prompt(self, text: &str) {
        let mut out = self.out();
        let _ = write!(out, "{text}");
        let _ = out.flush();
    }

    /// A follow-up the user must act on or should know about.
    pub fn notice(self, text: &str) {
        self.line(WARN, "note", text);
    }

    /// A loud warning about a security-relevant state the user must fix.
    pub fn warning(self, text: &str) {
        let _ = writeln!(self.err(), "{ERROR}WARNING{ERROR:#}: {text}");
    }

    /// Announce a finished host install.
    pub fn host_installed(self, layout: &Layout, distro: &str, port: u16, applied: usize) {
        self.line(
            GOOD,
            "installed",
            &format!(
                "host component of profile {} ({applied} changes; WSL distro {distro}, sshd port {port})",
                layout.profile
            ),
        );
        self.line(
            DIM,
            "journal",
            &layout.host_journal_path.display().to_string(),
        );
    }

    /// Print the plan of a dry run for one component; `holds[i]` says entry `i` is already in place.
    pub fn plan_component(self, label: &str, plan: &[Change], holds: Option<&[bool]>) {
        self.line(
            WARN,
            "dry run",
            &format!("{label} would apply {} changes:", plan.len()),
        );
        for (i, c) in plan.iter().enumerate() {
            let mark = match holds.and_then(|h| h.get(i)) {
                Some(true) => format!("{GOOD}in place{GOOD:#} "),
                Some(false) => format!("{WARN}will do {WARN:#} "),
                None => String::new(),
            };
            let _ = writeln!(self.out(), "  {DIM}{i:>2}{DIM:#} {mark}{}", describe(c));
        }
    }

    /// Print `status` rows.
    pub fn status(self, layout: &Layout, rows: &[StatusRow]) {
        self.line(DIM, "profile", &layout.profile);
        for r in rows {
            let (style, tag) = match (r.reverted, r.holds) {
                (true, _) => (DIM, "reverted"),
                (false, true) => (GOOD, "ok"),
                (false, false) => (WARN, "changed"),
            };
            self.line(style, tag, &describe(&r.change));
        }
    }

    /// Print that the profile is not installed.
    pub fn not_installed(self, layout: &Layout) {
        self.line(
            WARN,
            "absent",
            &format!("profile {} is not installed", layout.profile),
        );
    }
}
