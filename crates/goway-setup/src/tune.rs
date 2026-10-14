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
    /// Make this distro's virtual disk sparse (`wsl --manage DISTRO --set-sparse true`) so space
    /// freed inside WSL flows back to the Windows drive.
    pub sparse_distro: Option<String>,
}

impl TuneRequest {
    /// Whether nothing was asked for.
    pub fn is_empty(&self) -> bool {
        self.memory.is_none()
            && self.swap.is_none()
            && self.processors.is_none()
            && self.nested_virtualization.is_none()
            && self.sparse_distro.is_none()
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

/// The least memory, in MB, Windows keeps for itself when WSL is given the rest.
pub const MIN_HEADROOM_MB: u64 = 3 * 1024;

/// What Windows must keep, in MB, on a machine with `total_mb` of RAM: at least
/// [`MIN_HEADROOM_MB`], and a quarter of the RAM on larger machines.
///
/// The goway doctor applies the same rule to a helper's `.wslconfig` (`doctor/windows.rs`).
pub fn headroom_mb(total_mb: u64) -> u64 {
    MIN_HEADROOM_MB.max(total_mb / 4)
}

/// The largest WSL memory, in whole GB expressed in MB, that leaves [`headroom_mb`]; 0 when the
/// machine is too small to give WSL anything safely.
pub fn max_safe_memory_mb(total_mb: u64) -> u64 {
    total_mb.saturating_sub(headroom_mb(total_mb)) / 1024 * 1024
}

/// [`max_safe_memory_mb`] spelled the way `.wslconfig` takes it (`4GB`).
pub fn max_safe_memory(total_mb: u64) -> Result<String, SetupError> {
    match max_safe_memory_mb(total_mb) {
        0 => Err(SetupError::BadTuneValue(format!(
            "this machine has {} of RAM, too little to give WSL memory and keep {} for Windows",
            gb_text(total_mb),
            gb_text(headroom_mb(total_mb))
        ))),
        mb => Ok(format!("{}GB", mb / 1024)),
    }
}

/// `7.4 GB` style text for `mb` megabytes.
fn gb_text(mb: u64) -> String {
    #[allow(clippy::cast_precision_loss)] // display only
    let gb = mb as f64 / 1024.0;
    format!("{gb:.1} GB")
}

/// Refuse a `.wslconfig` memory (`8GB`, `2048MB`) that leaves Windows under the headroom on a
/// machine with `total_mb` of RAM, naming the RAM, the request and the largest safe size.
pub fn check_memory(memory: &str, total_mb: u64) -> Result<(), SetupError> {
    let Some(want_mb) = memory_mb(memory) else {
        return Ok(());
    };
    let safe_mb = max_safe_memory_mb(total_mb);
    if want_mb <= safe_mb {
        return Ok(());
    }
    tracing::warn!(
        memory,
        total_mb,
        safe_mb,
        "tune memory leaves Windows too little"
    );
    Err(SetupError::BadTuneValue(format!(
        "memory {memory} would leave Windows only {} of this machine's {} (it needs at least {}); the largest safe size is {}. Pass --force to set it anyway",
        gb_text(total_mb.saturating_sub(want_mb)),
        gb_text(total_mb),
        gb_text(headroom_mb(total_mb)),
        max_safe_memory(total_mb)?
    )))
}

/// Megabytes in a `.wslconfig` size such as `8GB` or `512MB`; `None` for anything else.
pub fn memory_mb(size: &str) -> Option<u64> {
    let t = size.trim();
    let end = t.find(|c: char| !c.is_ascii_digit()).unwrap_or(t.len());
    let n: u64 = t[..end].parse().ok()?;
    match t[end..].trim().to_ascii_uppercase().as_str() {
        "MB" | "M" => Some(n),
        "GB" | "G" => n.checked_mul(1024),
        _ => None,
    }
}

/// This machine's installed RAM in MB; `None` when it cannot be read (always off Windows).
#[cfg(windows)]
#[allow(unsafe_code)] // GlobalMemoryStatusEx FFI; see SAFETY
pub fn total_memory_mb() -> Option<u64> {
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    // SAFETY: an all-zero MEMORYSTATUSEX is valid plain data.
    let mut status: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
    status.dwLength = u32::try_from(std::mem::size_of::<MEMORYSTATUSEX>()).ok()?;
    // SAFETY: `status` is writable and its `dwLength` is set as the call requires.
    let ok = unsafe { GlobalMemoryStatusEx(&raw mut status) };
    (ok != 0).then_some(status.ullTotalPhys / (1024 * 1024))
}

/// This machine's installed RAM in MB; `None` when it cannot be read (always off Windows).
#[cfg(not(windows))]
pub fn total_memory_mb() -> Option<u64> {
    None
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
    if let Some(distro) = &req.sparse_distro {
        plan.push(crate::host::sparse_change(distro));
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

#[cfg(test)]
mod tests {
    use super::*;

    // frob:tests crates/goway-setup/src/tune.rs::max_safe_memory_mb
    #[test]
    fn headroom_is_three_gb_and_grows_on_large_machines() {
        assert_eq!(headroom_mb(7600), 3072);
        assert_eq!(headroom_mb(16 * 1024), 4096);
        assert_eq!(headroom_mb(64 * 1024), 16 * 1024);
        assert_eq!(max_safe_memory_mb(7600), 4096);
        assert_eq!(max_safe_memory_mb(2048), 0);
    }

    // frob:tests crates/goway-setup/src/tune.rs::check_memory
    #[test]
    fn a_size_that_starves_windows_is_refused_with_the_safe_size() {
        let err = check_memory("5632MB", 7600).unwrap_err().to_string();
        assert!(
            err.contains("7.4 GB") && err.contains("5632MB") && err.contains("4GB"),
            "{err}"
        );
        assert!(check_memory("4GB", 7600).is_ok());
        assert!(check_memory("0", 7600).is_ok());
    }
}
