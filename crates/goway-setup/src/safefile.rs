//! Reading and writing user-owned files from an elevated process without following links.
//!
//! The elevated child edits files the (unprivileged) user controls, `.wslconfig` above all. A
//! standard user with Developer Mode can turn such a file into a symbolic link, or a reparse
//! point, to a file only an administrator can touch; a plain `std::fs::write` would then write
//! through it with the administrator token. These helpers refuse a link or reparse point as
//! the final path component. On Windows the file is opened with `FILE_FLAG_OPEN_REPARSE_POINT`
//! and the attributes are read from the open handle, so there is no gap between the check and
//! the use; elsewhere (tests only) the check is a `symlink_metadata` look before opening.

use std::fs::{File, OpenOptions};
use std::io::{Read as _, Write as _};
use std::path::Path;

/// `FILE_FLAG_OPEN_REPARSE_POINT`: open the link itself instead of its target.
#[cfg(windows)]
const OPEN_REPARSE_POINT: u32 = 0x0020_0000;
/// `FILE_ATTRIBUTE_REPARSE_POINT`.
#[cfg(windows)]
const ATTRIBUTE_REPARSE_POINT: u32 = 0x400;

/// The error source of a refused link (so callers can tell it from other permission errors).
#[derive(Debug)]
pub struct LinkRefused(pub String);

impl std::fmt::Display for LinkRefused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} is a symbolic link or reparse point; refusing to follow it with administrator rights",
            self.0
        )
    }
}

impl std::error::Error for LinkRefused {}

/// Whether `e` is the refusal of a link.
pub fn is_link_refusal(e: &std::io::Error) -> bool {
    e.get_ref()
        .is_some_and(<dyn std::error::Error + Send + Sync>::is::<LinkRefused>)
}

fn refused(path: &Path) -> std::io::Error {
    tracing::error!(path = %path.display(), "refusing to follow a link or reparse point");
    std::io::Error::new(
        std::io::ErrorKind::PermissionDenied,
        LinkRefused(path.display().to_string()),
    )
}

/// Whether `file`, just opened without following reparse points, is itself a link.
fn is_link(path: &Path, file: &File) -> std::io::Result<bool> {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt as _;
        let _ = path;
        Ok(file.metadata()?.file_attributes() & ATTRIBUTE_REPARSE_POINT != 0)
    }
    #[cfg(not(windows))]
    {
        let _ = file;
        Ok(std::fs::symlink_metadata(path)?.file_type().is_symlink())
    }
}

fn open(path: &Path, options: &mut OpenOptions) -> std::io::Result<File> {
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt as _;
        options.custom_flags(OPEN_REPARSE_POINT);
    }
    #[cfg(not(windows))]
    if std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()) {
        // Opening with `create` would follow a dangling link and create its target.
        return Err(refused(path));
    }
    let file = options.open(path)?;
    if is_link(path, &file)? {
        return Err(refused(path));
    }
    Ok(file)
}

/// Read `path` as text; `Ok(None)` when it does not exist, an error when it is a link.
pub fn read_to_string(path: &Path) -> std::io::Result<Option<String>> {
    match open(path, OpenOptions::new().read(true)) {
        Ok(mut file) => {
            let mut text = String::new();
            file.read_to_string(&mut text)?;
            Ok(Some(text))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// Replace the contents of `path` (created when absent); an error when it is a link.
pub fn write(path: &Path, contents: &str) -> std::io::Result<()> {
    // Not truncated at open: the file is checked first, then emptied through the same handle.
    let mut file = open(
        path,
        OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false),
    )?;
    file.set_len(0)?;
    file.write_all(contents.as_bytes())?;
    file.flush()
}

/// Delete `path` when it is a file or a link (the link itself, never its target); absent is fine.
pub fn remove(path: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}
