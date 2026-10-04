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
    ActionKind, Change, Entry, Journal, JournalError, LocalSystem, Outcome, Prior, revert,
};

use crate::error::{Error, Result};
use crate::paths::Paths;
use crate::render::Renderer;

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
    let dir = file.parent().unwrap_or_else(|| Path::new("."));
    let log = journal_path(dir);
    if !dir.as_os_str().is_empty() {
        std::fs::create_dir_all(dir).map_err(|e| Error::io("create", dir, e))?;
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
        ) => format!(
            "{} {target} on {host} at unix time {at_unix_secs} ({reason})",
            kind.describe()
        ),
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
            reversible: !matches!(e.change, Change::Action { .. }),
            what: describe(e),
        })
        .collect())
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

/// Undo the newest `last` live entries of the change log in `dir` (all of them for `None`), newest
/// first. A file edited again since is left alone; an action is reported, never skipped silently.
/// The log file is removed once nothing in it is live.
pub fn undo(dir: &Path, last: Option<usize>) -> Result<Vec<Undone>> {
    let log = journal_path(dir);
    if !log.exists() {
        return Ok(Vec::new());
    }
    let mut journal = Journal::load(&log).map_err(|e| journal_err(&log, e))?;
    let live: Vec<usize> = (0..journal.entries.len())
        .filter(|&i| !journal.entries[i].reverted)
        .collect();
    let take = &live[live.len().saturating_sub(last.unwrap_or(live.len()))..];
    let mut part = Journal {
        entries: take.iter().map(|&i| journal.entries[i].clone()).collect(),
        ..journal.clone()
    };
    let report = revert(&mut part, &mut LocalSystem).map_err(|e| journal_err(&log, e))?;
    let mut done = Vec::new();
    for (k, outcome) in report.outcomes {
        let index = take[k];
        journal.entries[index].reverted = part.entries[k].reverted;
        done.push(Undone {
            number: index + 1,
            what: describe(&journal.entries[index]),
            outcome,
        });
    }
    if journal.entries.iter().all(|e| e.reverted) {
        std::fs::remove_file(&log).map_err(|e| Error::io("remove", &log, e))?;
        tracing::info!(path = %log.display(), "change log fully undone and removed");
    } else {
        journal.save(&log).map_err(|e| journal_err(&log, e))?;
    }
    Ok(done)
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
    let done = undo(&paths.config_dir, (!all).then_some(last))?;
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
