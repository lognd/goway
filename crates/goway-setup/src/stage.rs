//! A locked, hashed private copy of the setup exe for the UAC relaunch.
//!
//! The exe the user double-clicked sits in a user-writable folder (Downloads), and the install
//! path used to hand that very path to `ShellExecuteExW`: any process of the same account could
//! swap the file between the start of goway-setup and the elevation, and the UAC prompt would
//! then elevate the attacker's program. Instead the parent
//!
//! 1. copies the exe into a fresh, randomly named directory with an owner-only ACL,
//! 2. reopens the copy with a share mode that denies writing, renaming and deleting
//!    (`FILE_SHARE_READ` only) and **keeps that handle for the whole elevated lifetime**, so
//!    the bytes that run are the bytes that were hashed,
//! 3. hashes the copy through that handle and passes the SHA-256 on the elevated command line,
//!    where the elevated side hashes its own image again and refuses to continue on a mismatch
//!    (see [`verify_image`]).
//!
//! The one window left is between process start and step 1 (the file is read once, as the first
//! thing the install does, before the WSL precheck); a program the user did not start cannot be
//! told apart from the one they did without a code signature, which is out of scope here.

use std::io::Read as _;
use std::path::{Path, PathBuf};

use goway_journal::sha256_hex;

use crate::error::SetupError;

/// A private copy of the setup exe, locked against modification until dropped.
#[derive(Debug)]
pub struct StagedExe {
    dir: PathBuf,
    /// Path of the copy; the one to elevate.
    pub path: PathBuf,
    /// Lowercase hex SHA-256 of the copy.
    pub sha256: String,
    lock: Option<std::fs::File>,
}

fn untrusted(path: &Path, reason: impl Into<String>) -> SetupError {
    let reason = reason.into();
    tracing::error!(path = %path.display(), %reason, "refusing the setup exe");
    SetupError::UntrustedState {
        path: path.display().to_string(),
        reason,
    }
}

/// Open `path` for reading with a share mode that denies writers, renames and deletes.
fn open_locked(path: &Path) -> std::io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt as _;
        options.share_mode(0x1); // FILE_SHARE_READ only
    }
    options.open(path)
}

/// Create the fresh private directory the copy goes in.
#[cfg(windows)]
fn create_private_dir(dir: &Path) -> Result<(), SetupError> {
    let sid = crate::sysapi::current_user_sid().map_err(|e| SetupError::io(dir, e))?;
    // Protected, owner (the invoking user), SYSTEM and Administrators only.
    let sddl = format!("D:P(A;OICI;FA;;;{sid})(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)");
    crate::sysapi::create_dir_with_sddl(dir, &sddl).map_err(|e| SetupError::io(dir, e))
}

/// Create the fresh private directory the copy goes in (owner only).
#[cfg(not(windows))]
fn create_private_dir(dir: &Path) -> Result<(), SetupError> {
    use std::os::unix::fs::DirBuilderExt as _;
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(dir)
        .map_err(|e| SetupError::io(dir, e))
}

fn fresh_dir_name() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    format!("goway-elevate-{}-{nanos:x}", std::process::id())
}

/// Copy `exe` into a fresh private directory under `parent`, lock the copy and hash it.
pub fn stage_in(parent: &Path, exe: &Path) -> Result<StagedExe, SetupError> {
    let dir = parent.join(fresh_dir_name());
    create_private_dir(&dir)?;
    let result = stage_into(&dir, exe);
    if result.is_err() {
        let _ = std::fs::remove_dir_all(&dir);
    }
    result
}

fn stage_into(dir: &Path, exe: &Path) -> Result<StagedExe, SetupError> {
    let path = dir.join("goway-setup.exe");
    let mut source = std::fs::File::open(exe).map_err(|e| SetupError::io(exe, e))?;
    let mut original = Vec::new();
    source
        .read_to_end(&mut original)
        .map_err(|e| SetupError::io(exe, e))?;
    drop(source);
    std::fs::write(&path, &original).map_err(|e| SetupError::io(&path, e))?;
    // Lock first, hash through the lock: nothing can change the file after this point.
    let mut lock = open_locked(&path).map_err(|e| SetupError::io(&path, e))?;
    let mut locked = Vec::new();
    lock.read_to_end(&mut locked)
        .map_err(|e| SetupError::io(&path, e))?;
    let sha256 = sha256_hex(&locked);
    if sha256 != sha256_hex(&original) {
        return Err(untrusted(&path, "the staged copy differs from its source"));
    }
    tracing::info!(path = %path.display(), %sha256, "staged and locked the setup exe for elevation");
    Ok(StagedExe {
        dir: dir.to_path_buf(),
        path,
        sha256,
        lock: Some(lock),
    })
}

/// Stage `exe` under the user's temp directory.
pub fn stage(exe: &Path) -> Result<StagedExe, SetupError> {
    stage_in(&std::env::temp_dir(), exe)
}

impl Drop for StagedExe {
    fn drop(&mut self) {
        drop(self.lock.take());
        if let Err(e) = std::fs::remove_file(&self.path) {
            tracing::warn!(path = %self.path.display(), error = %e, "could not remove the staged setup exe");
        }
        if let Err(e) = std::fs::remove_dir(&self.dir) {
            tracing::warn!(dir = %self.dir.display(), error = %e, "could not remove the staging directory");
        }
    }
}

/// Whether `text` is a lowercase hex SHA-256 digest.
pub fn valid_sha256(text: &str) -> bool {
    text.len() == 64 && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Refuse to continue unless the file at `image` hashes to `expected` (what the elevated side
/// runs against the digest its parent computed through the locked copy).
pub fn verify_image(image: &Path, expected: &str) -> Result<(), SetupError> {
    if !valid_sha256(expected) {
        return Err(untrusted(image, "the expected exe digest is malformed"));
    }
    let bytes = std::fs::read(image).map_err(|e| SetupError::io(image, e))?;
    let got = sha256_hex(&bytes);
    if got == expected {
        tracing::info!(image = %image.display(), "the elevated exe matches the digest its parent computed");
        Ok(())
    } else {
        Err(untrusted(
            image,
            format!("the running exe hashes to {got}, not the {expected} its parent locked"),
        ))
    }
}
