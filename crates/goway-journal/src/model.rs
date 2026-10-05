//! In-memory `System` used to prove inversion in tests.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::change::{RegValue, ResourceKind};
use crate::error::SystemError;
use crate::system::{SysResult, System};

/// Mode reported for paths whose mode was never set.
pub const DEFAULT_MODE: u32 = 0o644;
/// SDDL reported for paths whose ACL was never set.
pub const DEFAULT_SDDL: &str = "";

/// A fully comparable in-memory machine; equality is state equality.
///
/// Mode and ACL maps hold only non-default values so equal states compare equal.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModelSystem {
    /// File contents by path.
    pub files: BTreeMap<PathBuf, String>,
    /// Existing directories.
    pub dirs: BTreeSet<PathBuf>,
    /// List variables by name.
    pub vars: BTreeMap<String, String>,
    /// Registry keys created explicitly (a key holding values also exists implicitly).
    pub reg_keys: BTreeSet<String>,
    /// Registry values by (key, name).
    pub registry: BTreeMap<(String, String), RegValue>,
    /// Non-default unix modes.
    pub modes: BTreeMap<PathBuf, u32>,
    /// Non-default SDDL strings.
    pub acls: BTreeMap<PathBuf, String>,
    /// External resources and their specs.
    pub resources: BTreeMap<(ResourceKind, String), String>,
    /// Resources that exist but are out of date, so an ensure replaces them.
    pub outdated: BTreeSet<(ResourceKind, String)>,
}

impl ModelSystem {
    /// An empty machine.
    pub fn new() -> Self {
        Self::default()
    }

    fn exists(&self, path: &Path) -> bool {
        self.files.contains_key(path) || self.dirs.contains(path)
    }

    fn require(&self, path: &Path) -> SysResult<()> {
        if self.exists(path) {
            Ok(())
        } else {
            Err(SystemError::NotFound(path.display().to_string()))
        }
    }

    fn parent_ok(&self, path: &Path) -> SysResult<()> {
        match path.parent() {
            Some(p)
                if p.parent().is_some() && !p.as_os_str().is_empty() && !self.dirs.contains(p) =>
            {
                Err(SystemError::NotFound(p.display().to_string()))
            }
            _ => Ok(()),
        }
    }
}

impl System for ModelSystem {
    fn read_file(&self, path: &Path) -> SysResult<Option<String>> {
        Ok(self.files.get(path).cloned())
    }

    fn write_file(&mut self, path: &Path, contents: &str) -> SysResult<()> {
        self.parent_ok(path)?;
        self.files.insert(path.to_path_buf(), contents.to_owned());
        Ok(())
    }

    fn remove_file(&mut self, path: &Path) -> SysResult<()> {
        if self.files.remove(path).is_some() {
            self.modes.remove(path);
            self.acls.remove(path);
        }
        Ok(())
    }

    fn dir_exists(&self, path: &Path) -> SysResult<bool> {
        Ok(self.dirs.contains(path))
    }

    fn create_dir(&mut self, path: &Path) -> SysResult<()> {
        self.parent_ok(path)?;
        self.dirs.insert(path.to_path_buf());
        Ok(())
    }

    fn dir_is_empty(&self, path: &Path) -> SysResult<bool> {
        let child = |p: &PathBuf| p.parent() == Some(path);
        Ok(!self.files.keys().any(child) && !self.dirs.iter().any(child))
    }

    fn remove_dir(&mut self, path: &Path) -> SysResult<()> {
        if !self.dirs.contains(path) {
            return Ok(());
        }
        if !self.dir_is_empty(path)? {
            return Err(SystemError::InvalidState(format!(
                "{} is not empty",
                path.display()
            )));
        }
        self.dirs.remove(path);
        self.modes.remove(path);
        self.acls.remove(path);
        Ok(())
    }

    fn get_var(&self, name: &str) -> SysResult<Option<String>> {
        Ok(self.vars.get(name).cloned())
    }

    fn set_var(&mut self, name: &str, value: &str) -> SysResult<()> {
        self.vars.insert(name.to_owned(), value.to_owned());
        Ok(())
    }

    fn remove_var(&mut self, name: &str) -> SysResult<()> {
        self.vars.remove(name);
        Ok(())
    }

    fn file_digest(&self, path: &Path) -> SysResult<Option<String>> {
        Ok(self
            .files
            .get(path)
            .map(|c| crate::digest::sha256_hex(c.as_bytes())))
    }

    fn copy_file(&mut self, src: &Path, dest: &Path) -> SysResult<()> {
        let contents = self
            .files
            .get(src)
            .cloned()
            .ok_or_else(|| SystemError::NotFound(src.display().to_string()))?;
        self.write_file(dest, &contents)
    }

    fn reg_key_exists(&self, key: &str) -> SysResult<bool> {
        Ok(self.reg_keys.contains(key) || self.registry.keys().any(|(k, _)| k == key))
    }

    fn reg_key_create(&mut self, key: &str) -> SysResult<()> {
        self.reg_keys.insert(key.to_owned());
        Ok(())
    }

    fn reg_key_is_empty(&self, key: &str) -> SysResult<bool> {
        Ok(!self.registry.keys().any(|(k, _)| k == key))
    }

    fn reg_key_remove(&mut self, key: &str) -> SysResult<()> {
        if !self.reg_key_is_empty(key)? {
            return Err(SystemError::InvalidState(format!("{key} is not empty")));
        }
        self.reg_keys.remove(key);
        Ok(())
    }

    fn reg_get(&self, key: &str, name: &str) -> SysResult<Option<RegValue>> {
        Ok(self
            .registry
            .get(&(key.to_owned(), name.to_owned()))
            .cloned())
    }

    fn reg_set(&mut self, key: &str, name: &str, value: &RegValue) -> SysResult<()> {
        self.registry
            .insert((key.to_owned(), name.to_owned()), value.clone());
        Ok(())
    }

    fn reg_delete(&mut self, key: &str, name: &str) -> SysResult<()> {
        self.registry.remove(&(key.to_owned(), name.to_owned()));
        Ok(())
    }

    fn get_mode(&self, path: &Path) -> SysResult<u32> {
        self.require(path)?;
        Ok(self.modes.get(path).copied().unwrap_or(DEFAULT_MODE))
    }

    fn set_mode(&mut self, path: &Path, mode: u32) -> SysResult<()> {
        self.require(path)?;
        if mode == DEFAULT_MODE {
            self.modes.remove(path);
        } else {
            self.modes.insert(path.to_path_buf(), mode);
        }
        Ok(())
    }

    fn get_acl(&self, path: &Path) -> SysResult<String> {
        self.require(path)?;
        Ok(self
            .acls
            .get(path)
            .cloned()
            .unwrap_or_else(|| DEFAULT_SDDL.to_owned()))
    }

    fn set_acl(&mut self, path: &Path, sddl: &str) -> SysResult<()> {
        self.require(path)?;
        if sddl == DEFAULT_SDDL {
            self.acls.remove(path);
        } else {
            self.acls.insert(path.to_path_buf(), sddl.to_owned());
        }
        Ok(())
    }

    fn resource_exists(&self, kind: ResourceKind, name: &str) -> SysResult<bool> {
        Ok(self.resources.contains_key(&(kind, name.to_owned())))
    }

    fn resource_create(&mut self, kind: ResourceKind, name: &str, spec: &str) -> SysResult<()> {
        self.outdated.remove(&(kind, name.to_owned()));
        self.resources
            .insert((kind, name.to_owned()), spec.to_owned());
        Ok(())
    }

    fn resource_outdated(&self, kind: ResourceKind, name: &str) -> SysResult<Option<String>> {
        let key = (kind, name.to_owned());
        Ok(self
            .outdated
            .contains(&key)
            .then(|| self.resources.get(&key).cloned().unwrap_or_default()))
    }

    fn resource_restore(
        &mut self,
        kind: ResourceKind,
        name: &str,
        snapshot: &str,
    ) -> SysResult<()> {
        self.resources
            .insert((kind, name.to_owned()), snapshot.to_owned());
        self.outdated.insert((kind, name.to_owned()));
        Ok(())
    }

    fn resource_delete(&mut self, kind: ResourceKind, name: &str) -> SysResult<()> {
        self.outdated.remove(&(kind, name.to_owned()));
        self.resources.remove(&(kind, name.to_owned()));
        Ok(())
    }
}
