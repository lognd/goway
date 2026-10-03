//! The administrator-only state directory (`%ProgramData%\goway\<profile>`).
//!
//! The host component's journal, settings, protected copy of the setup exe and elevated-run logs
//! live here, never under the user's profile: whatever an elevated process reads or replays must
//! come from a place only an administrator can write. The directory is created by the elevated
//! process with a protected ACL (Administrators and SYSTEM full control, users read only) and
//! every later read re-checks owner and ACL, refusing a directory that was pre-created or
//! loosened by someone else.

use std::path::{Path, PathBuf};

use crate::error::SetupError;

/// Security descriptor of the state directories: owner Administrators, protected (no inherited
/// entries), full control for Administrators and SYSTEM, read and traverse for Users.
pub const ADMIN_DIR_SDDL: &str = "O:BAD:P(A;OICI;FA;;;BA)(A;OICI;FA;;;SY)(A;OICI;0x1200a9;;;BU)";

/// Trustees that may own the directory or hold write access: Administrators, SYSTEM, `TrustedInstaller`.
const TRUSTED: [&str; 5] = [
    "BA",
    "SY",
    "S-1-5-32-544",
    "S-1-5-18",
    "S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464",
];

/// SDDL right tokens that cannot change anything (read, list, execute).
const READ_ONLY_TOKENS: [&str; 8] = ["GR", "GX", "FR", "FX", "RC", "RP", "LC", "LO"];

/// Access mask bits that change data, attributes, the ACL or the owner, or delete.
const WRITE_BITS: u32 = 0x0000_0002 // FILE_WRITE_DATA / ADD_FILE
    | 0x0000_0004 // FILE_APPEND_DATA / ADD_SUBDIRECTORY
    | 0x0000_0010 // FILE_WRITE_EA
    | 0x0000_0040 // FILE_DELETE_CHILD
    | 0x0000_0100 // FILE_WRITE_ATTRIBUTES
    | 0x0001_0000 // DELETE
    | 0x0004_0000 // WRITE_DAC
    | 0x0008_0000 // WRITE_OWNER
    | 0x1000_0000 // GENERIC_ALL
    | 0x4000_0000; // GENERIC_WRITE

fn trusted(sid: &str) -> bool {
    TRUSTED.contains(&sid)
}

/// Whether an ACE rights field grants nothing but reading.
fn read_only(rights: &str) -> bool {
    if let Some(hex) = rights
        .strip_prefix("0x")
        .or_else(|| rights.strip_prefix("0X"))
    {
        return u32::from_str_radix(hex, 16).is_ok_and(|mask| mask & WRITE_BITS == 0);
    }
    rights.len().is_multiple_of(2)
        && rights
            .as_bytes()
            .chunks(2)
            .all(|t| READ_ONLY_TOKENS.iter().any(|ok| ok.as_bytes() == t))
}

/// Check an owner-and-DACL SDDL string: the owner must be an administrative principal and no
/// other principal may be granted anything beyond reading. `Err` carries the reason.
pub fn check_sddl(sddl: &str) -> Result<(), String> {
    let header_end = sddl.find('(').unwrap_or(sddl.len());
    let header = &sddl[..header_end];
    let owner_start = header
        .find("O:")
        .ok_or_else(|| "no owner in the security descriptor".to_owned())?
        + 2;
    let owner_end = ["G:", "D:", "S:"]
        .iter()
        .filter_map(|marker| header[owner_start..].find(marker))
        .min()
        .map_or(header.len(), |i| owner_start + i);
    let owner = &header[owner_start..owner_end];
    if !trusted(owner) {
        return Err(format!("owned by {owner}, not by Administrators or SYSTEM"));
    }
    if !header[owner_end..].contains("D:") {
        return Err("no DACL (everyone would have full access)".to_owned());
    }
    for ace in sddl[header_end..]
        .split('(')
        .skip(1)
        .filter_map(|a| a.split(')').next())
    {
        let fields: Vec<&str> = ace.split(';').collect();
        let [kind, flags, rights, _guid, _inherit, sid, ..] = fields.as_slice() else {
            return Err(format!("unreadable access entry ({ace})"));
        };
        // Deny entries, trusted trustees and inherit-only entries (which apply to children
        // created later, not to the directory itself) cannot let anyone write to it.
        if matches!(*kind, "D" | "OD" | "XD") || trusted(sid) || flags.contains("IO") {
            continue;
        }
        if !read_only(rights) {
            return Err(format!("{sid} is granted {rights}, more than read access"));
        }
    }
    Ok(())
}

/// Wrap a reason as the refusal to trust `path`.
fn untrusted(path: &Path, reason: impl Into<String>) -> SetupError {
    let reason = reason.into();
    tracing::error!(path = %path.display(), %reason, "refusing to trust state directory");
    SetupError::UntrustedState {
        path: path.display().to_string(),
        reason,
    }
}

/// Refuse `dir` unless it is a real directory only administrators can write.
#[cfg(windows)]
pub fn verify(dir: &Path) -> Result<(), SetupError> {
    use std::os::windows::fs::MetadataExt as _;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    let meta = std::fs::symlink_metadata(dir).map_err(|e| SetupError::io(dir, e))?;
    if meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 || !meta.is_dir() {
        return Err(untrusted(dir, "it is a link or not a directory"));
    }
    let sddl = crate::sysapi::owner_and_dacl_sddl(dir).map_err(|e| SetupError::io(dir, e))?;
    tracing::debug!(dir = %dir.display(), %sddl, "checking state directory security");
    check_sddl(&sddl).map_err(|reason| untrusted(dir, reason))
}

/// Refuse `dir` unless it is a real directory no other user can write (unix stand-in).
#[cfg(not(windows))]
pub fn verify(dir: &Path) -> Result<(), SetupError> {
    use std::os::unix::fs::PermissionsExt as _;
    let meta = std::fs::symlink_metadata(dir).map_err(|e| SetupError::io(dir, e))?;
    if meta.file_type().is_symlink() || !meta.is_dir() {
        return Err(untrusted(dir, "it is a symlink or not a directory"));
    }
    if meta.permissions().mode() & 0o022 != 0 {
        return Err(untrusted(dir, "it is writable by group or others"));
    }
    Ok(())
}

/// Create `dir` with the protected ACL when absent (only an elevated process can), then verify it.
///
/// A directory somebody else pre-created (a standard user can create folders in `ProgramData`)
/// would make every install fail forever. An empty one is removed and re-created properly,
/// since nothing in it can matter; one with contents is refused with the exact way out.
pub fn ensure(dir: &Path) -> Result<(), SetupError> {
    match ensure_once(dir) {
        Err(SetupError::UntrustedState { reason, .. }) if is_empty_real_dir(dir) => {
            tracing::warn!(dir = %dir.display(), %reason, "replacing an empty, untrusted state directory");
            std::fs::remove_dir(dir).map_err(|e| SetupError::io(dir, e))?;
            ensure_once(dir)
        }
        Err(SetupError::UntrustedState { path, reason }) => Err(SetupError::UntrustedState {
            path,
            reason: format!(
                "{reason}. It was probably created by another user. As an administrator, look inside it and, if nothing there is yours, delete it (in an elevated prompt: rd /s /q \"{}\"), then run goway-setup again",
                dir.display()
            ),
        }),
        other => other,
    }
}

/// Whether `dir` is a real (not linked) empty directory.
fn is_empty_real_dir(dir: &Path) -> bool {
    std::fs::symlink_metadata(dir)
        .is_ok_and(|m| m.is_dir() && !m.file_type().is_symlink() && !is_reparse(&m))
        && std::fs::read_dir(dir).is_ok_and(|mut entries| entries.next().is_none())
}

#[cfg(windows)]
fn is_reparse(meta: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt as _;
    meta.file_attributes() & 0x400 != 0
}

#[cfg(not(windows))]
fn is_reparse(_meta: &std::fs::Metadata) -> bool {
    false
}

#[cfg(windows)]
fn ensure_once(dir: &Path) -> Result<(), SetupError> {
    match crate::sysapi::create_dir_with_sddl(dir, ADMIN_DIR_SDDL) {
        Ok(()) => {
            tracing::info!(dir = %dir.display(), "created administrator-only state directory");
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            tracing::debug!(dir = %dir.display(), "state directory exists; verifying it");
        }
        Err(e) => return Err(SetupError::io(dir, e)),
    }
    verify(dir)
}

/// Create `dir` readable and writable by its owner only when absent, then verify it (unix stand-in).
#[cfg(not(windows))]
fn ensure_once(dir: &Path) -> Result<(), SetupError> {
    use std::os::unix::fs::DirBuilderExt as _;
    if let Some(parent) = dir.parent() {
        std::fs::create_dir_all(parent).map_err(|e| SetupError::io(parent, e))?;
    }
    match std::fs::DirBuilder::new().mode(0o755).create(dir) {
        Ok(()) => tracing::info!(dir = %dir.display(), "created state directory"),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(SetupError::io(dir, e)),
    }
    verify(dir)
}

/// File name prefix of the logs an elevated run writes.
pub const LOG_PREFIX: &str = "elevated-";

/// Accept only a plain log file name (no separators, no traversal) of the expected shape.
pub fn valid_log_name(name: &str) -> bool {
    name.starts_with(LOG_PREFIX)
        && name.strip_suffix(".log").is_some()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.'))
        && !name.contains("..")
}

/// A fresh log file name for one elevated run (unique per process and instant).
pub fn new_log_name() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    format!("{LOG_PREFIX}{}-{nanos:x}.log", std::process::id())
}

/// Delete elevated-run logs in `dir` other than `keep`; failures are logged, never fatal.
pub fn remove_old_logs(dir: &Path, keep: Option<&str>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if valid_log_name(&name)
            && Some(name.as_str()) != keep
            && let Err(e) = std::fs::remove_file(entry.path())
        {
            tracing::warn!(log = %name, error = %e, "could not remove an old elevated log");
        }
    }
}

/// Path of the protected copy of the setup exe kept for elevated runs.
pub fn protected_exe(dir: &Path) -> PathBuf {
    dir.join("bin").join("goway-setup.exe")
}

/// Copy `exe` to the protected location in `dir` and check the copy byte for byte; elevated runs
/// of later commands start this copy instead of the user-writable installed one.
pub fn install_protected_exe(dir: &Path, exe: &Path) -> Result<PathBuf, SetupError> {
    let target = protected_exe(dir);
    let parent = target.parent().unwrap_or(dir);
    std::fs::create_dir_all(parent).map_err(|e| SetupError::io(parent, e))?;
    if target.exists() {
        std::fs::remove_file(&target).map_err(|e| SetupError::io(&target, e))?;
    }
    std::fs::copy(exe, &target).map_err(|e| SetupError::io(&target, e))?;
    let want = std::fs::read(exe).map_err(|e| SetupError::io(exe, e))?;
    let got = std::fs::read(&target).map_err(|e| SetupError::io(&target, e))?;
    if want != got {
        return Err(untrusted(
            &target,
            "the protected copy differs from its source",
        ));
    }
    tracing::info!(target = %target.display(), "installed the protected setup copy");
    Ok(target)
}

/// Remove everything goway keeps in `dir` (the protected exe, logs, settings, journal) and the
/// directory itself plus `root` when empty. `keep_log` (the log this process is writing) and
/// the exe when `running` is the protected copy cannot go while in use. Returns whether
/// everything is gone; when not, the caller schedules the removal for after it exits.
pub fn purge(dir: &Path, root: &Path, running: &Path, keep_log: Option<&str>) -> bool {
    remove_old_logs(dir, keep_log);
    let bin = dir.join("bin");
    let exe = protected_exe(dir);
    if !same_file(&exe, running) {
        if let Err(e) = std::fs::remove_file(&exe) {
            tracing::debug!(path = %exe.display(), error = %e, "no protected exe to remove");
        }
        let _ = std::fs::remove_dir(&bin);
    }
    let mut gone = true;
    for path in [dir, root] {
        match std::fs::remove_dir(path) {
            Ok(()) => tracing::info!(path = %path.display(), "removed state directory"),
            Err(e) => {
                tracing::debug!(path = %path.display(), error = %e, "kept state directory");
                gone = false;
            }
        }
    }
    gone
}

/// Whether two paths name the same file (case-insensitively, as Windows does).
pub fn same_file(a: &Path, b: &Path) -> bool {
    let norm = |p: &Path| {
        let s = p.to_string_lossy().replace('/', "\\").to_lowercase();
        s.strip_prefix("\\\\?\\").map_or(s.clone(), str::to_owned)
    };
    norm(a) == norm(b)
}
