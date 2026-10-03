//! The real-filesystem `System`: files, directories and unix modes.

use std::io::ErrorKind;
use std::path::Path;

use crate::change::{RegValue, ResourceKind};
use crate::error::SystemError;
use crate::system::{SysResult, System};

/// The host machine. Files, directories, unix modes and (on Windows) the registry and user
/// environment variables work; ACLs and resources are not yet supported.
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

/// The public key file of a private key path.
fn public_key(private: &Path) -> std::path::PathBuf {
    let mut name = private.as_os_str().to_owned();
    name.push(".pub");
    std::path::PathBuf::from(name)
}

/// Create an unencrypted ed25519 key pair with `ssh-keygen`.
fn create_key_pair(private: &Path, comment: &str) -> SysResult<()> {
    tracing::info!(key = %private.display(), comment, "local: create ssh key pair");
    let out = std::process::Command::new("ssh-keygen")
        .args(["-q", "-t", "ed25519", "-N", "", "-C", comment, "-f"])
        .arg(private)
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| io(private, e))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(SystemError::InvalidState(format!(
            "ssh-keygen failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )))
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

    fn file_digest(&self, path: &Path) -> SysResult<Option<String>> {
        match std::fs::read(path) {
            Ok(b) => Ok(Some(crate::digest::sha256_hex(&b))),
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
            Err(e) => Err(io(path, e)),
        }
    }

    fn copy_file(&mut self, src: &Path, dest: &Path) -> SysResult<()> {
        tracing::debug!(src = %src.display(), dest = %dest.display(), "local: copy file");
        std::fs::copy(src, dest)
            .map(drop)
            .map_err(|e| io_or_missing(dest, e))
    }

    #[cfg(windows)]
    fn get_var(&self, name: &str) -> SysResult<Option<String>> {
        crate::local_windows::get_var(name)
    }

    #[cfg(not(windows))]
    fn get_var(&self, _name: &str) -> SysResult<Option<String>> {
        Err(SystemError::Unsupported("list variables"))
    }

    #[cfg(windows)]
    fn set_var(&mut self, name: &str, value: &str) -> SysResult<()> {
        crate::local_windows::set_var(name, value)
    }

    #[cfg(not(windows))]
    fn set_var(&mut self, _name: &str, _value: &str) -> SysResult<()> {
        Err(SystemError::Unsupported("list variables"))
    }

    #[cfg(windows)]
    fn remove_var(&mut self, name: &str) -> SysResult<()> {
        crate::local_windows::remove_var(name)
    }

    #[cfg(not(windows))]
    fn remove_var(&mut self, _name: &str) -> SysResult<()> {
        Err(SystemError::Unsupported("list variables"))
    }

    #[cfg(windows)]
    fn reg_key_exists(&self, key: &str) -> SysResult<bool> {
        crate::local_windows::reg_key_exists(key)
    }

    #[cfg(not(windows))]
    fn reg_key_exists(&self, _key: &str) -> SysResult<bool> {
        Err(SystemError::Unsupported("registry"))
    }

    #[cfg(windows)]
    fn reg_key_create(&mut self, key: &str) -> SysResult<()> {
        crate::local_windows::reg_key_create(key)
    }

    #[cfg(not(windows))]
    fn reg_key_create(&mut self, _key: &str) -> SysResult<()> {
        Err(SystemError::Unsupported("registry"))
    }

    #[cfg(windows)]
    fn reg_key_is_empty(&self, key: &str) -> SysResult<bool> {
        crate::local_windows::reg_key_is_empty(key)
    }

    #[cfg(not(windows))]
    fn reg_key_is_empty(&self, _key: &str) -> SysResult<bool> {
        Err(SystemError::Unsupported("registry"))
    }

    #[cfg(windows)]
    fn reg_key_remove(&mut self, key: &str) -> SysResult<()> {
        crate::local_windows::reg_key_remove(key)
    }

    #[cfg(not(windows))]
    fn reg_key_remove(&mut self, _key: &str) -> SysResult<()> {
        Err(SystemError::Unsupported("registry"))
    }

    #[cfg(windows)]
    fn reg_get(&self, key: &str, name: &str) -> SysResult<Option<RegValue>> {
        crate::local_windows::reg_get(key, name)
    }

    #[cfg(not(windows))]
    fn reg_get(&self, _key: &str, _name: &str) -> SysResult<Option<RegValue>> {
        Err(SystemError::Unsupported("registry"))
    }

    #[cfg(windows)]
    fn reg_set(&mut self, key: &str, name: &str, value: &RegValue) -> SysResult<()> {
        crate::local_windows::reg_set(key, name, value)
    }

    #[cfg(not(windows))]
    fn reg_set(&mut self, _key: &str, _name: &str, _value: &RegValue) -> SysResult<()> {
        Err(SystemError::Unsupported("registry"))
    }

    #[cfg(windows)]
    fn reg_delete(&mut self, key: &str, name: &str) -> SysResult<()> {
        crate::local_windows::reg_delete(key, name)
    }

    #[cfg(not(windows))]
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

    fn resource_exists(&self, kind: ResourceKind, name: &str) -> SysResult<bool> {
        match kind {
            ResourceKind::SshKeyPair => {
                let private = Path::new(name);
                Ok(private.is_file() && public_key(private).is_file())
            }
            _ => Err(SystemError::Unsupported("external resources")),
        }
    }

    fn resource_create(&mut self, kind: ResourceKind, name: &str, spec: &str) -> SysResult<()> {
        match kind {
            ResourceKind::SshKeyPair => create_key_pair(Path::new(name), spec),
            _ => Err(SystemError::Unsupported("external resources")),
        }
    }

    fn resource_delete(&mut self, kind: ResourceKind, name: &str) -> SysResult<()> {
        match kind {
            ResourceKind::SshKeyPair => {
                let private = Path::new(name);
                tracing::info!(key = %private.display(), "local: delete ssh key pair");
                for path in [private.to_path_buf(), public_key(private)] {
                    match std::fs::remove_file(&path) {
                        Ok(()) => {}
                        Err(e) if e.kind() == ErrorKind::NotFound => {}
                        Err(e) => return Err(io(&path, e)),
                    }
                }
                Ok(())
            }
            _ => Err(SystemError::Unsupported("external resources")),
        }
    }
}
