//! Pure planning: turn a change (or a recorded entry) plus current state into primitive ops.

use std::path::{Path, PathBuf};

use crate::change::{Change, ListPosition, RegValue, ResourceKind};
use crate::error::JournalError;
use crate::journal::{Entry, Prior};
use crate::linefile::{LineFile, find_key, find_section, header_name, parse_kv};
use crate::system::System;

/// One primitive mutation, executed after the entry is recorded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Op {
    WriteFile(PathBuf, String),
    RemoveFile(PathBuf),
    CreateDir(PathBuf),
    /// Remove directories innermost first, stopping at the first that is missing-not-empty.
    RemoveEmptyDirs(Vec<PathBuf>),
    SetVar(String, String),
    RemoveVar(String),
    RegSet(String, String, RegValue),
    RegDelete(String, String),
    SetMode(PathBuf, u32),
    SetAcl(PathBuf, String),
    CreateResource(ResourceKind, String, String),
    DeleteResource(ResourceKind, String),
}

/// What happened to one entry during revert.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The prior state was restored.
    Restored,
    /// The entry recorded a no-op, so nothing was done.
    Noop,
    /// The entry was already reverted earlier.
    AlreadyReverted,
    /// The target no longer holds what goway wrote (or is gone); left untouched.
    LeftAlone(String),
}

/// Run ops against a system.
pub(crate) fn run(sys: &mut (impl System + ?Sized), ops: &[Op]) -> Result<(), JournalError> {
    for op in ops {
        tracing::debug!(?op, "executing op");
        match op {
            Op::WriteFile(p, c) => sys.write_file(p, c)?,
            Op::RemoveFile(p) => sys.remove_file(p)?,
            Op::CreateDir(p) => sys.create_dir(p)?,
            Op::RemoveEmptyDirs(dirs) => {
                for p in dirs {
                    if sys.dir_exists(p)? {
                        if !sys.dir_is_empty(p)? {
                            tracing::info!(path = %p.display(), "directory not empty, keeping it and its parents");
                            break;
                        }
                        sys.remove_dir(p)?;
                    }
                }
            }
            Op::SetVar(n, v) => sys.set_var(n, v)?,
            Op::RemoveVar(n) => sys.remove_var(n)?,
            Op::RegSet(k, n, v) => sys.reg_set(k, n, v)?,
            Op::RegDelete(k, n) => sys.reg_delete(k, n)?,
            Op::SetMode(p, m) => sys.set_mode(p, *m)?,
            Op::SetAcl(p, s) => sys.set_acl(p, s)?,
            Op::CreateResource(k, n, s) => sys.resource_create(*k, n, s)?,
            Op::DeleteResource(k, n) => sys.resource_delete(*k, n)?,
        }
    }
    Ok(())
}

fn noop() -> (Prior, Vec<Op>) {
    (Prior::Noop, Vec::new())
}

fn invalid(msg: impl Into<String>) -> JournalError {
    JournalError::Invalid(msg.into())
}

fn split_list(raw: &str, sep: char) -> Vec<String> {
    if raw.is_empty() {
        Vec::new()
    } else {
        raw.split(sep).map(str::to_owned).collect()
    }
}

fn join_list(parts: &[String], sep: char) -> String {
    parts.join(&sep.to_string())
}

/// Tagged line text: `<line> <marker>` (just `<line>` for an empty marker).
fn tagged(line: &str, marker: &str) -> String {
    if marker.is_empty() {
        line.to_owned()
    } else {
        format!("{line} {marker}")
    }
}

/// Write `lf` back, or remove the file when goway created it and nothing is left.
fn store(path: &Path, lf: &LineFile, remove_if_empty: bool) -> Op {
    if remove_if_empty && lf.lines.is_empty() {
        Op::RemoveFile(path.to_path_buf())
    } else {
        Op::WriteFile(path.to_path_buf(), lf.render())
    }
}

/// Capture the prior state of `change` and the ops that realise it.
#[allow(clippy::too_many_lines)] // one flat arm per change kind reads better than splitting
pub(crate) fn plan_apply(
    change: &Change,
    sys: &(impl System + ?Sized),
) -> Result<(Prior, Vec<Op>), JournalError> {
    match change {
        Change::WriteFile { path, contents } => {
            let prior = sys.read_file(path)?;
            if prior.as_deref() == Some(contents) {
                return Ok(noop());
            }
            Ok((
                Prior::File { contents: prior },
                vec![Op::WriteFile(path.clone(), contents.clone())],
            ))
        }
        Change::EnsureLine { path, line, marker } => {
            if line.contains('\n') || marker.contains('\n') {
                return Err(invalid("line or marker contains a newline"));
            }
            let text = tagged(line, marker);
            let existing = sys.read_file(path)?;
            let mut lf = LineFile::parse(existing.as_deref().unwrap_or(""));
            if lf.lines.contains(&text) {
                return Ok(noop());
            }
            let at = lf.lines.len();
            let fixed_newline = lf.insert(at, text);
            let prior = Prior::Line {
                created_file: existing.is_none(),
                fixed_newline,
            };
            Ok((prior, vec![Op::WriteFile(path.clone(), lf.render())]))
        }
        Change::EnsureDir { path } => {
            let mut missing = Vec::new();
            let mut cur = Some(path.as_path());
            while let Some(p) = cur {
                if p.parent().is_none() || p.as_os_str().is_empty() || sys.dir_exists(p)? {
                    break;
                }
                missing.push(p.to_path_buf());
                cur = p.parent();
            }
            if missing.is_empty() {
                return Ok(noop());
            }
            missing.reverse();
            let ops = missing.iter().cloned().map(Op::CreateDir).collect();
            Ok((Prior::DirsCreated { created: missing }, ops))
        }
        Change::EnsureListEntry {
            var,
            entry,
            separator,
            position,
        } => {
            if entry.is_empty() || entry.contains(*separator) {
                return Err(invalid("list entry is empty or contains the separator"));
            }
            let current = sys.get_var(var)?;
            let mut parts = split_list(current.as_deref().unwrap_or(""), *separator);
            if parts.contains(entry) {
                return Ok(noop());
            }
            match position {
                ListPosition::Front => parts.insert(0, entry.clone()),
                ListPosition::Back => parts.push(entry.clone()),
            }
            let ops = vec![Op::SetVar(var.clone(), join_list(&parts, *separator))];
            Ok((
                Prior::ListEntry {
                    created_var: current.is_none(),
                },
                ops,
            ))
        }
        Change::SetRegistryValue { key, name, value } => {
            let prior = sys.reg_get(key, name)?;
            if prior.as_ref() == Some(value) {
                return Ok(noop());
            }
            Ok((
                Prior::Registry { value: prior },
                vec![Op::RegSet(key.clone(), name.clone(), value.clone())],
            ))
        }
        Change::SetIniKey {
            path,
            section,
            key,
            value,
        } => plan_ini_apply(sys, path, section, key, value),
        Change::SetUnixMode { path, mode } => {
            let prior = sys.get_mode(path)?;
            if prior == *mode {
                return Ok(noop());
            }
            Ok((
                Prior::Mode { mode: prior },
                vec![Op::SetMode(path.clone(), *mode)],
            ))
        }
        Change::SetAcl { path, sddl } => {
            let prior = sys.get_acl(path)?;
            if prior == *sddl {
                return Ok(noop());
            }
            Ok((
                Prior::Acl { sddl: prior },
                vec![Op::SetAcl(path.clone(), sddl.clone())],
            ))
        }
        Change::EnsureResource { kind, name, spec } => {
            if sys.resource_exists(*kind, name)? {
                return Ok(noop());
            }
            Ok((
                Prior::ResourceCreated,
                vec![Op::CreateResource(*kind, name.clone(), spec.clone())],
            ))
        }
    }
}

fn plan_ini_apply(
    sys: &(impl System + ?Sized),
    path: &Path,
    section: &str,
    key: &str,
    value: &str,
) -> Result<(Prior, Vec<Op>), JournalError> {
    if [section, key, value].iter().any(|s| s.contains('\n'))
        || key.is_empty()
        || section.is_empty()
    {
        return Err(invalid(
            "ini section, key or value is empty or contains a newline",
        ));
    }
    let existing = sys.read_file(path)?;
    let created_file = existing.is_none();
    let mut lf = LineFile::parse(existing.as_deref().unwrap_or(""));
    let new_line = format!("{key}={value}");
    let prior;
    if let Some((h, e)) = find_section(&lf.lines, section) {
        if let Some(i) = find_key(&lf.lines, h + 1..e, key) {
            if parse_kv(&lf.lines[i]).is_some_and(|(_, v)| v == value) {
                return Ok(noop());
            }
            let original_line = std::mem::replace(&mut lf.lines[i], new_line);
            prior = Prior::IniReplaced { original_line };
        } else {
            let at = (h + 1..e)
                .rev()
                .find(|&i| !lf.lines[i].trim().is_empty())
                .unwrap_or(h)
                + 1;
            let fixed_newline = lf.insert(at, new_line);
            prior = Prior::IniInserted {
                created_file,
                created_section: false,
                added_blank: false,
                fixed_newline,
            };
        }
    } else {
        let added_blank = lf.lines.last().is_some_and(|l| !l.trim().is_empty());
        let mut fixed_newline = false;
        if added_blank {
            let at = lf.lines.len();
            fixed_newline = lf.insert(at, String::new());
        }
        let at = lf.lines.len();
        fixed_newline |= lf.insert(at, format!("[{section}]"));
        let at = lf.lines.len();
        lf.insert(at, new_line);
        prior = Prior::IniInserted {
            created_file,
            created_section: true,
            added_blank,
            fixed_newline,
        };
    }
    Ok((prior, vec![Op::WriteFile(path.to_path_buf(), lf.render())]))
}

/// Plan the inverse of `entry` against the current state.
#[allow(clippy::too_many_lines)] // one flat arm per change kind reads better than splitting
pub(crate) fn plan_revert(
    entry: &Entry,
    sys: &(impl System + ?Sized),
) -> Result<(Outcome, Vec<Op>), JournalError> {
    let left = |why: &str| Ok((Outcome::LeftAlone(why.to_owned()), Vec::new()));
    let restore = |ops| Ok((Outcome::Restored, ops));
    match (&entry.change, &entry.prior) {
        (_, Prior::Noop) => Ok((Outcome::Noop, Vec::new())),
        (Change::WriteFile { path, contents }, Prior::File { contents: before }) => {
            if sys.read_file(path)?.as_deref() != Some(contents) {
                return left("file no longer holds the written contents");
            }
            restore(vec![match before {
                Some(b) => Op::WriteFile(path.clone(), b.clone()),
                None => Op::RemoveFile(path.clone()),
            }])
        }
        (
            Change::EnsureLine { path, line, marker },
            Prior::Line {
                created_file,
                fixed_newline,
            },
        ) => {
            let Some(text) = sys.read_file(path)? else {
                return left("file is gone");
            };
            let mut lf = LineFile::parse(&text);
            let want = tagged(line, marker);
            let Some(i) = lf.lines.iter().rposition(|l| *l == want) else {
                return left("line is no longer present");
            };
            lf.remove(i);
            if *fixed_newline {
                lf.unfix_newline(i);
            }
            restore(vec![store(path, &lf, *created_file)])
        }
        (Change::EnsureDir { .. }, Prior::DirsCreated { created }) => {
            let innermost_first: Vec<PathBuf> = created.iter().rev().cloned().collect();
            restore(vec![Op::RemoveEmptyDirs(innermost_first)])
        }
        (
            Change::EnsureListEntry {
                var,
                entry: item,
                separator,
                position,
            },
            Prior::ListEntry { created_var },
        ) => {
            let Some(raw) = sys.get_var(var)? else {
                return left("variable is gone");
            };
            let mut parts = split_list(&raw, *separator);
            let found = match position {
                ListPosition::Front => parts.iter().position(|p| p == item),
                ListPosition::Back => parts.iter().rposition(|p| p == item),
            };
            let Some(i) = found else {
                return left("entry is no longer present");
            };
            parts.remove(i);
            if parts.is_empty() && *created_var {
                restore(vec![Op::RemoveVar(var.clone())])
            } else {
                restore(vec![Op::SetVar(var.clone(), join_list(&parts, *separator))])
            }
        }
        (Change::SetRegistryValue { key, name, value }, Prior::Registry { value: before }) => {
            if sys.reg_get(key, name)?.as_ref() != Some(value) {
                return left("registry value changed since install");
            }
            restore(vec![match before {
                Some(b) => Op::RegSet(key.clone(), name.clone(), b.clone()),
                None => Op::RegDelete(key.clone(), name.clone()),
            }])
        }
        (
            Change::SetIniKey {
                path,
                section,
                key,
                value,
            },
            p @ (Prior::IniReplaced { .. } | Prior::IniInserted { .. }),
        ) => plan_ini_revert(sys, path, section, key, value, p),
        (Change::SetUnixMode { path, mode }, Prior::Mode { mode: before }) => {
            if sys.get_mode(path).ok() != Some(*mode) {
                return left("mode changed since install");
            }
            restore(vec![Op::SetMode(path.clone(), *before)])
        }
        (Change::SetAcl { path, sddl }, Prior::Acl { sddl: before }) => {
            if sys.get_acl(path).ok().as_deref() != Some(sddl.as_str()) {
                return left("ACL changed since install");
            }
            restore(vec![Op::SetAcl(path.clone(), before.clone())])
        }
        (Change::EnsureResource { kind, name, .. }, Prior::ResourceCreated) => {
            if !sys.resource_exists(*kind, name)? {
                return left("resource is gone");
            }
            restore(vec![Op::DeleteResource(*kind, name.clone())])
        }
        (change, prior) => Err(invalid(format!(
            "entry prior {prior:?} does not match change {change:?}"
        ))),
    }
}

fn plan_ini_revert(
    sys: &(impl System + ?Sized),
    path: &Path,
    section: &str,
    key: &str,
    value: &str,
    prior: &Prior,
) -> Result<(Outcome, Vec<Op>), JournalError> {
    let left = |why: &str| Ok((Outcome::LeftAlone(why.to_owned()), Vec::new()));
    let Some(text) = sys.read_file(path)? else {
        return left("file is gone");
    };
    let mut lf = LineFile::parse(&text);
    let Some((h, e)) = find_section(&lf.lines, section) else {
        return left("section is gone");
    };
    let Some(i) = find_key(&lf.lines, h + 1..e, key) else {
        return left("key is gone");
    };
    if parse_kv(&lf.lines[i]).is_none_or(|(_, v)| v != value) {
        return left("key changed since install");
    }
    let created_file = match prior {
        Prior::IniReplaced { original_line } => {
            lf.lines[i].clone_from(original_line);
            false
        }
        Prior::IniInserted {
            created_file,
            created_section,
            added_blank,
            fixed_newline,
        } => {
            lf.remove(i);
            if *created_section {
                // The section is ours to drop only when nothing else lives in it.
                let body_empty = lf.lines.get(h + 1).is_none_or(|l| header_name(l).is_some());
                if body_empty {
                    lf.remove(h);
                    let mut first = h;
                    if *added_blank && h > 0 && lf.lines[h - 1].trim().is_empty() {
                        lf.remove(h - 1);
                        first = h - 1;
                    }
                    if *fixed_newline {
                        lf.unfix_newline(first);
                    }
                }
            } else if *fixed_newline {
                lf.unfix_newline(i);
            }
            *created_file
        }
        _ => unreachable!("plan_ini_revert is only called for ini priors"),
    };
    Ok((Outcome::Restored, vec![store(path, &lf, created_file)]))
}
