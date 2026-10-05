//! `goway-setup tune`: give the WSL helper more of the machine (memory, swap, processors,
//! nested virtualization) through journaled `.wslconfig` edits.
//!
//! `.wslconfig` is the user's own file, so tuning needs no administrator rights and keeps its own
//! user-level journal (`tune-journal.json` next to the client journal). The elevated host
//! uninstall never reads it: `uninstall --host` reverts it first, in the user's own process. The
//! values only apply after `wsl --shutdown`, which stops every WSL process, so the command asks
//! first and never runs while goway jobs are running.

use std::path::{Path, PathBuf};

use goway_journal::{Change, Journal, System, apply_with, revert};

use crate::error::SetupError;

/// Section of `.wslconfig` that holds the VM settings.
pub const WSL2_SECTION: &str = "wsl2";

/// What the user asked to change; `None` leaves a setting alone.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TuneRequest {
    /// `memory`, as `.wslconfig` spells it (`8GB`).
    pub memory: Option<String>,
    /// `swap` (`0` or `4GB`).
    pub swap: Option<String>,
    /// `processors`.
    pub processors: Option<u32>,
    /// `nestedVirtualization`.
    pub nested_virtualization: Option<bool>,
}

impl TuneRequest {
    /// Whether nothing was asked for.
    pub fn is_empty(&self) -> bool {
        self.memory.is_none()
            && self.swap.is_none()
            && self.processors.is_none()
            && self.nested_virtualization.is_none()
    }
}

/// Parse a size such as `8GB`, `8 G`, `512mb` or `0` into the spelling `.wslconfig` takes.
///
/// Whole numbers only, in MB or GB: at most 1 TB, and at least 512 MB unless it is `0` (swap).
pub fn parse_size(text: &str) -> Result<String, SetupError> {
    let bad = |why: &str| SetupError::BadTuneValue(format!("size {text:?}: {why}"));
    let t = text.trim();
    if t == "0" {
        return Ok("0".to_owned());
    }
    let digits_end = t.find(|c: char| !c.is_ascii_digit()).unwrap_or(t.len());
    let (num, unit) = t.split_at(digits_end);
    let n: u64 = num.parse().map_err(|_| bad("expected a whole number"))?;
    let mb = match unit.trim().to_ascii_uppercase().as_str() {
        "MB" | "M" => n,
        "GB" | "G" => n.checked_mul(1024).ok_or_else(|| bad("too large"))?,
        _ => return Err(bad("use MB or GB, for example 8GB")),
    };
    if !(512..=1024 * 1024).contains(&mb) {
        return Err(bad("must be between 512MB and 1024GB (or 0)"));
    }
    if unit.trim().to_ascii_uppercase().starts_with('G') {
        Ok(format!("{n}GB"))
    } else {
        Ok(format!("{n}MB"))
    }
}

/// Check a processor count: 1 to 1024.
pub fn parse_processors(n: u32) -> Result<u32, SetupError> {
    if (1..=1024).contains(&n) {
        Ok(n)
    } else {
        Err(SetupError::BadTuneValue(format!(
            "processors {n}: must be between 1 and 1024"
        )))
    }
}

/// The journaled `.wslconfig` changes for `req`, in a fixed order.
pub fn tune_plan(wslconfig: &Path, req: &TuneRequest) -> Vec<Change> {
    let set = |key: &str, value: String| Change::SetIniKey {
        path: wslconfig.to_path_buf(),
        section: WSL2_SECTION.to_owned(),
        key: key.to_owned(),
        value,
    };
    let mut plan = Vec::new();
    if let Some(v) = &req.memory {
        plan.push(set("memory", v.clone()));
    }
    if let Some(v) = &req.swap {
        plan.push(set("swap", v.clone()));
    }
    if let Some(v) = req.processors {
        plan.push(set("processors", v.to_string()));
    }
    if let Some(v) = req.nested_virtualization {
        plan.push(set("nestedVirtualization", v.to_string()));
    }
    plan
}

/// `<home>\.wslconfig`.
pub fn wslconfig_path(home: &Path) -> PathBuf {
    home.join(".wslconfig")
}

/// Apply `plan` onto the tune journal at `path` (created when absent), saving after every entry;
/// a failure reverts what this call did (and everything earlier, as the journal is replayed
/// backwards) is left as it was.
pub fn apply_tune(
    sys: &mut (impl System + ?Sized),
    path: &Path,
    plan: &[Change],
) -> Result<Journal, SetupError> {
    let existing = if path.exists() {
        Journal::load(path)?
    } else {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| SetupError::io(dir, e))?;
        }
        Journal::generate()
    };
    let before = existing.entries.len();
    let save_to = path.to_path_buf();
    match apply_with(plan, sys, existing, &mut |j| j.save(&save_to)) {
        Ok(j) => Ok(j),
        Err(e) => {
            tracing::error!(index = e.index, error = %e.source, "tune failed; rolling back");
            let mut journal = *e.journal;
            let mut mine = Journal {
                entries: journal.entries.split_off(before),
                ..journal.clone()
            };
            revert(&mut mine, sys)?;
            journal.save(path)?;
            Err(SetupError::TuneFailed(e.source.to_string()))
        }
    }
}

/// Revert and delete the tune journal at `path`; `Ok(false)` when there is none.
pub fn revert_tune(sys: &mut (impl System + ?Sized), path: &Path) -> Result<bool, SetupError> {
    Ok(revert_tune_reported(sys, path)?.is_some())
}

/// [`revert_tune`], returning the per-entry outcomes so a caller can tell the person about the
/// recorded actions (a WSL restart) that undo cannot reverse; `None` when there is no journal.
pub fn revert_tune_reported(
    sys: &mut (impl System + ?Sized),
    path: &Path,
) -> Result<Option<goway_journal::RevertReport>, SetupError> {
    if !path.exists() {
        return Ok(None);
    }
    let mut journal = Journal::load(path)?;
    let report = revert(&mut journal, sys)?;
    std::fs::remove_file(path).map_err(|e| SetupError::io(path, e))?;
    tracing::info!(path = %path.display(), "tune journal reverted and removed");
    Ok(Some(report))
}
