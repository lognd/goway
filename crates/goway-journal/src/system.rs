//! The primitive operations a `System` must provide.

use std::path::Path;

use crate::change::{RegValue, ResourceKind};
use crate::error::SystemError;

/// Shorthand for results of primitive operations.
pub type SysResult<T> = Result<T, SystemError>;

/// The primitive read/write operations journaled changes are built from.
///
/// Removal operations on absent targets succeed, so replays are safe to repeat.
pub trait System {
    /// Read a file; `None` when it does not exist.
    fn read_file(&self, path: &Path) -> SysResult<Option<String>>;
    /// Create or truncate a file with `contents` (the parent directory must exist).
    fn write_file(&mut self, path: &Path, contents: &str) -> SysResult<()>;
    /// Remove a file; absent is success.
    fn remove_file(&mut self, path: &Path) -> SysResult<()>;
    /// Whether a directory exists at `path`.
    fn dir_exists(&self, path: &Path) -> SysResult<bool>;
    /// Create exactly one directory level (the parent must exist).
    fn create_dir(&mut self, path: &Path) -> SysResult<()>;
    /// Whether an existing directory has no entries.
    fn dir_is_empty(&self, path: &Path) -> SysResult<bool>;
    /// Remove an empty directory; absent is success, non-empty is an error.
    fn remove_dir(&mut self, path: &Path) -> SysResult<()>;
    /// Read a list variable (its raw joined string); `None` when unset.
    fn get_var(&self, name: &str) -> SysResult<Option<String>>;
    /// Set a list variable to a raw joined string.
    fn set_var(&mut self, name: &str, value: &str) -> SysResult<()>;
    /// Unset a list variable; absent is success.
    fn remove_var(&mut self, name: &str) -> SysResult<()>;
    /// Lowercase hex SHA-256 of a file's bytes; `None` when it does not exist.
    fn file_digest(&self, path: &Path) -> SysResult<Option<String>>;
    /// Copy the bytes of `src` to `dest` (the parent of `dest` must exist).
    fn copy_file(&mut self, src: &Path, dest: &Path) -> SysResult<()>;
    /// Whether a registry-like key exists.
    fn reg_key_exists(&self, key: &str) -> SysResult<bool>;
    /// Create a registry-like key and its missing ancestors.
    fn reg_key_create(&mut self, key: &str) -> SysResult<()>;
    /// Whether an existing key holds no values and no subkeys.
    fn reg_key_is_empty(&self, key: &str) -> SysResult<bool>;
    /// Remove a key that holds no values and no subkeys; absent is success, non-empty is an error.
    fn reg_key_remove(&mut self, key: &str) -> SysResult<()>;
    /// Read a registry-like value.
    fn reg_get(&self, key: &str, name: &str) -> SysResult<Option<RegValue>>;
    /// Write a registry-like value.
    fn reg_set(&mut self, key: &str, name: &str, value: &RegValue) -> SysResult<()>;
    /// Delete a registry-like value; absent is success.
    fn reg_delete(&mut self, key: &str, name: &str) -> SysResult<()>;
    /// Read the unix permission bits of an existing path.
    fn get_mode(&self, path: &Path) -> SysResult<u32>;
    /// Set the unix permission bits of an existing path.
    fn set_mode(&mut self, path: &Path, mode: u32) -> SysResult<()>;
    /// Read the SDDL of an existing path.
    fn get_acl(&self, path: &Path) -> SysResult<String>;
    /// Set the SDDL of an existing path.
    fn set_acl(&mut self, path: &Path, sddl: &str) -> SysResult<()>;
    /// Whether a named external resource exists.
    fn resource_exists(&self, kind: ResourceKind, name: &str) -> SysResult<bool>;
    /// Create a named external resource from an opaque spec.
    fn resource_create(&mut self, kind: ResourceKind, name: &str, spec: &str) -> SysResult<()>;
    /// Delete a named external resource; absent is success.
    fn resource_delete(&mut self, kind: ResourceKind, name: &str) -> SysResult<()>;
}
