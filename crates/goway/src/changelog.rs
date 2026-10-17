//! The change log of this laptop's goway: every change goway makes to a machine, listed and undoable.
//!
//! One `changes.json` (a `goway-journal` journal) sits in goway's config directory. It holds the
//! edits of goway's config and pinned `known_hosts` as whole-file writes with the text they
//! replaced, and the actions that cannot be inverted (a fix command run on a helper, a package
//! installed here) with time, host and reason. `goway changes` lists it and `goway changes undo`
//! reverts it, newest first; actions are reported as not reversible, never skipped silently.
//!
//! goway's own run state (work trees, caches, locks, state files) is not a machine change and is
//! not recorded here; `goway gc` owns it. See `docs/changes.md`.

use std::path::{Path, PathBuf};

use goway_journal::{
    ActionKind, Change, Entry, Journal, JournalError, LocalSystem, Outcome, Prior, RevertReport,
    revert,
};

use crate::error::{Error, Result};
use crate::paths::Paths;
use crate::render::Renderer;
use crate::spawn::CommandExt as _;

/// The name of the change log inside goway's config directory.
pub const FILE_NAME: &str = "changes.json";

/// The host name recorded for changes made on this laptop.
pub const LOCAL_HOST: &str = "localhost";

/// Where the change log of the config directory `dir` lives.
pub fn journal_path(dir: &Path) -> PathBuf {
    dir.join(FILE_NAME)
}

fn journal_err(path: &Path, source: JournalError) -> Error {
    Error::Journal {
        path: path.to_owned(),
        source,
    }
}

/// Write `bytes` (text) to `file` atomically, after recording the write in the change log beside
/// it with the text it replaces, so the edit can be listed and undone.
///
/// The record is saved first: when it cannot be, the file is not touched.
pub fn write_file(file: &Path, bytes: &[u8]) -> Result<()> {
    write_file_in(file.parent().unwrap_or_else(|| Path::new(".")), file, bytes)
}

/// Like [`write_file`], but the record goes into the change log of `log_dir` (the config
/// directory), so a file under a subdirectory still shows in `goway changes`.
pub fn write_file_in(log_dir: &Path, file: &Path, bytes: &[u8]) -> Result<()> {
    let after = std::str::from_utf8(bytes)
        .map_err(|e| Error::Usage(format!("{} would not be text: {e}", file.display())))?;
    let before = match std::fs::read_to_string(file) {
        Ok(text) => Some(text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(Error::io("read", file, e)),
    };
    if before.as_deref() == Some(after) {
        tracing::debug!(file = %file.display(), "unchanged; nothing to record");
        return Ok(());
    }
    let log = journal_path(log_dir);
    let dir = file.parent().unwrap_or_else(|| Path::new("."));
    if !dir.as_os_str().is_empty() {
        std::fs::create_dir_all(dir).map_err(|e| Error::io("create", dir, e))?;
    }
    if !log_dir.as_os_str().is_empty() {
        std::fs::create_dir_all(log_dir).map_err(|e| Error::io("create", log_dir, e))?;
    }
    Journal::record_write_at(&log, file, before, after).map_err(|e| journal_err(&log, e))?;
    crate::config::write_atomic(file, bytes)
}

/// Record, before it runs, an action on `host` that undo cannot reverse (a fix command, a package
/// install, a started task); `undo` says how a person takes it back by hand, when they can.
///
/// When this fails the action must not run: a change the log does not hold cannot be listed.
pub fn record_action(
    dir: &Path,
    kind: ActionKind,
    target: &str,
    host: &str,
    reason: &str,
    undo: Option<&str>,
) -> Result<()> {
    let log = journal_path(dir);
    std::fs::create_dir_all(dir).map_err(|e| Error::io("create", dir, e))?;
    tracing::info!(
        ?kind,
        target,
        host,
        reason,
        "recording an action in the change log"
    );
    Journal::record_action_at(&log, kind, target, host, reason, undo)
        .map_err(|e| journal_err(&log, e))
}

/// One line of the change log as `goway changes` shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    /// Position in the log (1-based, oldest first).
    pub number: usize,
    /// Whether the entry was undone (or reported as not reversible) already.
    pub settled: bool,
    /// Whether undo can reverse it.
    pub reversible: bool,
    /// What it changed, in words.
    pub what: String,
}

/// Seconds since the unix epoch as `YYYY-MM-DD HH:MM UTC`.
fn utc_time(secs: u64) -> String {
    let at = std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs);
    let stamp = humantime::format_rfc3339_seconds(at)
        .to_string()
        .replace('T', " ");
    format!("{} UTC", stamp.get(..16).unwrap_or(&stamp))
}

/// Seconds since the unix epoch as a local `YYYY-MM-DD HH:MM`, from the system's `date`; UTC
/// (and labelled so) when this machine has no `date` that converts a stamp.
fn local_time(secs: u64) -> String {
    let attempts = [("-d", format!("@{secs}")), ("-r", secs.to_string())];
    for (flag, arg) in attempts {
        let out = std::process::Command::new("date")
            .args([flag, &arg, "+%Y-%m-%d %H:%M"])
            .stdin(std::process::Stdio::null())
            .output_locked();
        if let Ok(out) = out
            && out.status.success()
        {
            let text = String::from_utf8_lossy(&out.stdout).trim().to_owned();
            if text.len() == 16 {
                return text;
            }
        }
    }
    tracing::debug!(secs, "no local time from `date`; showing UTC");
    utc_time(secs)
}

/// The tool and (when the download URL names one) version a recorded fix script installs as a
/// pinned tool under `~/.local/opt/goway-TOOL`; `None` for any other script.
fn pinned_install(script: &str) -> Option<(String, Option<String>)> {
    if !script.contains("tar xf") {
        return None;
    }
    let tool = script.split("goway-").skip(1).find_map(|rest| {
        let (name, _) = rest.split_once('"')?;
        (!name.is_empty()
            && name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'))
        .then(|| name.to_owned())
    })?;
    let url = script
        .split_once("curl -fsSL ")
        .and_then(|(_, rest)| rest.split_whitespace().next())
        .unwrap_or_default();
    let version = url
        .split('/')
        .flat_map(|segment| segment.split(['-', '_']))
        .map(|part| part.strip_prefix('v').unwrap_or(part))
        .find(|part| {
            part.contains('.')
                && part.starts_with(|c: char| c.is_ascii_digit())
                && part.chars().all(|c| c.is_ascii_digit() || c == '.')
        })
        .map(str::to_owned);
    Some((tool, version))
}

/// The helper and install script of an entry that records a pinned-tool install by `doctor --fix`
/// (the install itself is journaled in the helper's fix journal and undoable from here).
fn pinned_of(entry: &Entry) -> Option<(String, String)> {
    let Change::Action {
        kind: ActionKind::RunFix,
        target,
        host,
        ..
    } = &entry.change
    else {
        return None;
    };
    pinned_install(target)?;
    Some((host.clone(), target.clone()))
}

/// The index of the live pinned-tool entry for `script` in the fix journal of `host`.
fn live_fix(dir: &Path, host: &str, script: &str) -> Option<usize> {
    let log = crate::doctor::fixes_path(dir, host);
    let fixes = Journal::load(&log).ok()?;
    fixes.entries.iter().position(|e| {
        !e.reverted && matches!(&e.change, Change::EnsureResource { spec, .. } if spec == script)
    })
}

/// The first line of a script, cut to a length that fits a table.
fn brief(script: &str) -> String {
    const MAX: usize = 60;
    let line = script.lines().next().unwrap_or_default().trim();
    if line.chars().count() > MAX || script.lines().count() > 1 {
        let head: String = line.chars().take(MAX).collect();
        format!("{head}...")
    } else {
        line.to_owned()
    }
}

fn describe(entry: &Entry) -> String {
    match (&entry.change, &entry.prior) {
        (Change::WriteFile { path, .. }, _) => format!("wrote {}", path.display()),
        (
            Change::Action {
                kind,
                target,
                host,
                reason,
                ..
            },
            Prior::Action { at_unix_secs },
        ) => {
            let when = local_time(*at_unix_secs);
            if let Some((tool, version)) = pinned_of(entry).and_then(|(_, s)| pinned_install(&s)) {
                let version = version.map_or_else(String::new, |v| format!(" {v}"));
                return format!("installed {tool}{version} user-level on {host} at {when}");
            }
            let target = if matches!(kind, ActionKind::RunFix) {
                format!("`{}`", brief(target))
            } else {
                target.clone()
            };
            format!(
                "{} {target} on {host} at {when} ({reason})",
                kind.describe()
            )
        }
        (other, _) => format!("{other:?}"),
    }
}

/// The rows of the change log in `dir`; empty when there is none.
pub fn rows(dir: &Path) -> Result<Vec<Row>> {
    let log = journal_path(dir);
    if !log.exists() {
        return Ok(Vec::new());
    }
    let journal = Journal::load(&log).map_err(|e| journal_err(&log, e))?;
    Ok(journal
        .entries
        .iter()
        .enumerate()
        .map(|(i, e)| Row {
            number: i + 1,
            settled: e.reverted,
            reversible: match pinned_of(e) {
                Some((host, script)) => live_fix(dir, &host, &script).is_some(),
                None => !matches!(e.change, Change::Action { .. }),
            },
            what: describe(e),
        })
        .collect())
}

/// How many entries the change log in `dir` holds (0 when there is none).
pub fn len(dir: &Path) -> Result<usize> {
    let log = journal_path(dir);
    if !log.exists() {
        return Ok(0);
    }
    Ok(Journal::load(&log)
        .map_err(|e| journal_err(&log, e))?
        .entries
        .len())
}

/// Settle the entries from position `from` (0-based) on, because an exact inverse of them has
/// just been performed by other means (`goway ssh setup --undo` puts the config back itself);
/// removes the log file once nothing in it is live. Entries before `from` stay live.
pub fn settle_from(dir: &Path, from: usize) -> Result<()> {
    let log = journal_path(dir);
    if !log.exists() {
        return Ok(());
    }
    let mut journal = Journal::load(&log).map_err(|e| journal_err(&log, e))?;
    for entry in journal.entries.iter_mut().skip(from) {
        entry.reverted = true;
    }
    tracing::info!(from, "change log entries settled by an exact inverse");
    if journal.entries.iter().all(|e| e.reverted) {
        std::fs::remove_file(&log).map_err(|e| Error::io("remove", &log, e))
    } else {
        journal.save(&log).map_err(|e| journal_err(&log, e))
    }
}

/// What `undo` did with one entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Undone {
    /// Position in the log (1-based).
    pub number: usize,
    /// What it changed, in words.
    pub what: String,
    /// What undo did.
    pub outcome: Outcome,
}

/// Reverts a part of a helper's fix journal on that helper; `Err` says why it could not.
pub type RemoteRevert<'a> =
    dyn FnMut(&str, &mut Journal) -> std::result::Result<RevertReport, String> + 'a;

/// Undo the newest `last` live entries of the change log in `dir` (all of them for `None`), newest
/// first, with no way to reach a helper: a pinned-tool install is reported as not undone. A file
/// edited again since is left alone; an action is reported, never skipped silently. The log file
/// is removed once nothing in it is live.
pub fn undo(dir: &Path, last: Option<usize>) -> Result<Vec<Undone>> {
    undo_with(dir, last, &mut |host, _| {
        Err(format!("{host} is not reachable from here"))
    })
}

/// [`undo`], reverting a pinned-tool install on its helper through `remote` (the install is
/// journaled in the helper's fix journal, whose entry is reverted and the journal kept or removed
/// as `goway uninstall` would).
pub fn undo_with(
    dir: &Path,
    last: Option<usize>,
    remote: &mut RemoteRevert<'_>,
) -> Result<Vec<Undone>> {
    let log = journal_path(dir);
    if !log.exists() {
        return Ok(Vec::new());
    }
    let mut journal = Journal::load(&log).map_err(|e| journal_err(&log, e))?;
    let live: Vec<usize> = (0..journal.entries.len())
        .filter(|&i| !journal.entries[i].reverted)
        .collect();
    let take = &live[live.len().saturating_sub(last.unwrap_or(live.len()))..];
    let (fixes, local): (Vec<usize>, Vec<usize>) = take
        .iter()
        .partition(|&&i| pinned_of(&journal.entries[i]).is_some());
    let mut done = Vec::new();
    if !local.is_empty() {
        let mut part = Journal {
            entries: local.iter().map(|&i| journal.entries[i].clone()).collect(),
            ..journal.clone()
        };
        let report = revert(&mut part, &mut LocalSystem).map_err(|e| journal_err(&log, e))?;
        for (k, outcome) in report.outcomes {
            let index = local[k];
            journal.entries[index].reverted = part.entries[k].reverted;
            done.push(Undone {
                number: index + 1,
                what: describe(&journal.entries[index]),
                outcome,
            });
        }
    }
    for &index in fixes.iter().rev() {
        let step = undo_fix(dir, &mut journal, index, remote)?;
        done.push(step);
    }
    done.sort_by_key(|step| std::cmp::Reverse(step.number));
    if journal.entries.iter().all(|e| e.reverted) {
        std::fs::remove_file(&log).map_err(|e| Error::io("remove", &log, e))?;
        tracing::info!(path = %log.display(), "change log fully undone and removed");
    } else {
        journal.save(&log).map_err(|e| journal_err(&log, e))?;
    }
    Ok(done)
}

/// Undo the pinned-tool install recorded at `index` of `journal` through the helper's fix journal;
/// the entry is settled when the tool is gone (or already was), kept live when it could not be
/// taken back.
fn undo_fix(
    dir: &Path,
    journal: &mut Journal,
    index: usize,
    remote: &mut RemoteRevert<'_>,
) -> Result<Undone> {
    let what = describe(&journal.entries[index]);
    let (host, script) = pinned_of(&journal.entries[index]).unwrap_or_default();
    let (outcome, settled) = match live_fix(dir, &host, &script) {
        None => {
            tracing::info!(%host, "no live pinned-tool entry in the fix journal; nothing to take back");
            (Outcome::Noop, true)
        }
        Some(k) => {
            let log = crate::doctor::fixes_path(dir, &host);
            let mut fixes = Journal::load(&log).map_err(|e| journal_err(&log, e))?;
            let mut part = Journal {
                entries: vec![fixes.entries[k].clone()],
                ..fixes.clone()
            };
            match remote(&host, &mut part) {
                Err(why) => {
                    tracing::warn!(%host, %why, "pinned tool not removed");
                    (Outcome::NotReversible(format!("{what} ({why})")), false)
                }
                Ok(report) => {
                    fixes.entries[k].reverted = part.entries[0].reverted;
                    if fixes.entries.iter().all(|e| e.reverted) {
                        std::fs::remove_file(&log).map_err(|e| Error::io("remove", &log, e))?;
                    } else {
                        fixes.save(&log).map_err(|e| journal_err(&log, e))?;
                    }
                    let outcome = report
                        .outcomes
                        .into_iter()
                        .next()
                        .map_or(Outcome::Noop, |(_, o)| o);
                    (outcome, part.entries[0].reverted)
                }
            }
        }
    };
    journal.entries[index].reverted = settled;
    Ok(Undone {
        number: index + 1,
        what,
        outcome,
    })
}

/// The production [`RemoteRevert`]: reaches the helper named in the config and reverts there.
fn reach_helper(
    paths: &Paths,
) -> impl FnMut(&str, &mut Journal) -> std::result::Result<RevertReport, String> + use<> {
    let paths = paths.clone();
    move |name, journal| {
        let config =
            crate::config::Config::load(&paths.config_file()).map_err(|e| e.to_string())?;
        let host = config.host(name).map_err(|e| e.to_string())?;
        let settings = crate::ssh::Settings::from_paths(&paths);
        let mut state =
            crate::state::State::load(&paths.state_file()).map_err(|e| e.to_string())?;
        let prober = crate::resolve::SshProber {
            settings: settings.clone(),
        };
        let found = crate::resolve::resolve_call(
            &config,
            host,
            &mut state,
            &crate::resolve::SystemLookup,
            &prober,
            crate::ssh::KeyPolicy::Strict,
            &crate::remote::Call::new("ping", &[] as &[&str]),
        )
        .map_err(|e| e.to_string())?;
        let mut sys = crate::remotesys::RemoteSystem {
            target: found.target.clone(),
            settings,
            password: false,
            prompt: None,
        };
        revert(journal, &mut sys).map_err(|e| e.to_string())
    }
}

/// `goway changes`: list every recorded change, oldest first.
pub fn show(paths: &Paths, renderer: Renderer) -> Result<u8> {
    let rows = rows(&paths.config_dir)?;
    if rows.is_empty() {
        renderer.note("no changes are recorded");
        return Ok(0);
    }
    let mut table = vec![vec![
        "#".to_owned(),
        "state".to_owned(),
        "undo".to_owned(),
        "change".to_owned(),
    ]];
    for row in rows {
        table.push(vec![
            row.number.to_string(),
            if row.settled { "undone" } else { "active" }.to_owned(),
            if row.reversible { "yes" } else { "no" }.to_owned(),
            row.what,
        ]);
    }
    renderer.table(&table);
    renderer.next(
        "`goway changes undo` reverts the newest active change (`--last N` or `--all` for more)",
    );
    Ok(0)
}

/// `goway changes undo`: revert the newest `last` changes, or all of them.
pub fn undo_command(paths: &Paths, renderer: Renderer, last: usize, all: bool) -> Result<u8> {
    let done = undo_with(
        &paths.config_dir,
        (!all).then_some(last),
        &mut reach_helper(paths),
    )?;
    if done.is_empty() {
        renderer.note("nothing to undo");
        return Ok(0);
    }
    let mut code = 0;
    for step in done {
        match step.outcome {
            Outcome::Restored => renderer.ok(format_args!("undone: {}", step.what)),
            Outcome::Noop | Outcome::AlreadyReverted => {
                renderer.note(format_args!("nothing to do: {}", step.what));
            }
            Outcome::LeftAlone(why) => {
                code = 1;
                renderer.warn(format_args!("kept: {} ({why})", step.what));
            }
            Outcome::NotReversible(what) => {
                code = 1;
                renderer.warn(format_args!("not undone: {what}"));
            }
        }
    }
    Ok(code)
}
