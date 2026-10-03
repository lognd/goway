//! The real-filesystem `System`: files, directories and unix modes.

use std::io::ErrorKind;
use std::path::Path;

use crate::change::{RegValue, ResourceKind};
use crate::error::SystemError;
use crate::system::{SysResult, System};

/// The host machine. Variables, registry, ACLs and resources are not yet supported.
#[derive(Debug, Clone, Copy, Default)]
pub struct LocalSystem;

fn io(path: &Path, source: std::io::Error) -> SystemError {
    SystemError::Io {
        path: path.to_path_buf(),
        source,
    }
}

fn io_or_missing(path: &Path, source: std::io::Error) -> SystemError {
    if source.kind() == ErrorKind::NotFound {
        SystemError::NotFound(path.display().to_string())
    } else {
        io(path, source)
    }
}

impl System for LocalSystem {
    fn read_file(&self, path: &Path) -> SysResult<Option<String>> {
        match std::fs::read_to_string(path) {
            Ok(s) => Ok(Some(s)),
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
            Err(e) => Err(io(path, e)),
        }
    }

    fn write_file(&mut self, path: &Path, contents: &str) -> SysResult<()> {
        tracing::debug!(path = %path.display(), "local: write file");
        std::fs::write(path, contents).map_err(|e| io(path, e))
    }

    fn remove_file(&mut self, path: &Path) -> SysResult<()> {
        tracing::debug!(path = %path.display(), "local: remove file");
        match std::fs::remove_file(path) {
            Err(e) if e.kind() != ErrorKind::NotFound => Err(io(path, e)),
            _ => Ok(()),
        }
    }

    fn dir_exists(&self, path: &Path) -> SysResult<bool> {
        Ok(path.is_dir())
    }

    fn create_dir(&mut self, path: &Path) -> SysResult<()> {
        tracing::debug!(path = %path.display(), "local: create dir");
        std::fs::create_dir(path).map_err(|e| io_or_missing(path, e))
    }

    fn dir_is_empty(&self, path: &Path) -> SysResult<bool> {
        let mut entries = std::fs::read_dir(path).map_err(|e| io_or_missing(path, e))?;
        Ok(entries.next().is_none())
    }

    fn remove_dir(&mut self, path: &Path) -> SysResult<()> {
        tracing::debug!(path = %path.display(), "local: remove dir");
        match std::fs::remove_dir(path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(()),
            Err(e) if e.kind() == ErrorKind::DirectoryNotEmpty => Err(SystemError::InvalidState(
                format!("{} is not empty", path.display()),
            )),
            Err(e) => Err(io(path, e)),
        }
    }

    fn get_var(&self, _name: &str) -> SysResult<Option<String>> {
        Err(SystemError::Unsupported("list variables"))
    }

    fn set_var(&mut self, _name: &str, _value: &str) -> SysResult<()> {
        Err(SystemError::Unsupported("list variables"))
    }

    fn remove_var(&mut self, _name: &str) -> SysResult<()> {
        Err(SystemError::Unsupported("list variables"))
    }

    fn reg_get(&self, _key: &str, _name: &str) -> SysResult<Option<RegValue>> {
        Err(SystemError::Unsupported("registry"))
    }

    fn reg_set(&mut self, _key: &str, _name: &str, _value: &RegValue) -> SysResult<()> {
        Err(SystemError::Unsupported("registry"))
    }

    fn reg_delete(&mut self, _key: &str, _name: &str) -> SysResult<()> {
        Err(SystemError::Unsupported("registry"))
    }

    #[cfg(unix)]
    fn get_mode(&self, path: &Path) -> SysResult<u32> {
        use std::os::unix::fs::PermissionsExt;
        let meta = std::fs::metadata(path).map_err(|e| io_or_missing(path, e))?;
        Ok(meta.permissions().mode() & 0o7777)
    }

    #[cfg(not(unix))]
    fn get_mode(&self, _path: &Path) -> SysResult<u32> {
        Err(SystemError::Unsupported("unix modes"))
    }

    #[cfg(unix)]
    fn set_mode(&mut self, path: &Path, mode: u32) -> SysResult<()> {
        use std::os::unix::fs::PermissionsExt;
        tracing::debug!(path = %path.display(), mode = format_args!("{mode:o}"), "local: set mode");
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
            .map_err(|e| io_or_missing(path, e))
    }

    #[cfg(not(unix))]
    fn set_mode(&mut self, _path: &Path, _mode: u32) -> SysResult<()> {
        Err(SystemError::Unsupported("unix modes"))
    }

    fn get_acl(&self, _path: &Path) -> SysResult<String> {
        Err(SystemError::Unsupported("ACLs"))
    }

    fn set_acl(&mut self, _path: &Path, _sddl: &str) -> SysResult<()> {
        Err(SystemError::Unsupported("ACLs"))
    }

    fn resource_exists(&self, _kind: ResourceKind, _name: &str) -> SysResult<bool> {
        Err(SystemError::Unsupported("external resources"))
    }

    fn resource_create(&mut self, _kind: ResourceKind, _name: &str, _spec: &str) -> SysResult<()> {
        Err(SystemError::Unsupported("external resources"))
    }

    fn resource_delete(&mut self, _kind: ResourceKind, _name: &str) -> SysResult<()> {
        Err(SystemError::Unsupported("external resources"))
    }
}
