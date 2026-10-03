//! A journal `System` whose files live on a remote host, reached over ssh.
//!
//! Used by `goway ssh setup` so the changes it makes on the host
//! (`~/.ssh`, `authorized_keys`, their modes) are journaled and undone the
//! same way the installers' local changes are. Only file, directory and
//! mode operations are supported; files are replaced atomically and keep
//! their mode.

use std::io::Write as _;
use std::path::Path;
use std::process::Stdio;

use goway_journal::{RegValue, ResourceKind, SysResult, System, SystemError};

use crate::ssh::{self, KeyPolicy, Target};

/// Files on `target`; `password` lets ssh prompt (first-time setup).
#[derive(Debug, Clone)]
pub struct RemoteSystem {
    /// Where.
    pub target: Target,
    /// ssh settings (`ControlMaster` keeps one password login for all calls).
    pub settings: ssh::Settings,
    /// Allow password authentication (interactive).
    pub password: bool,
}

/// Outcome of one remote shell snippet.
struct Out {
    code: Option<i32>,
    stdout: Vec<u8>,
    stderr: String,
}

impl RemoteSystem {
    fn exec(&self, script: &str, input: Option<&[u8]>) -> SysResult<Out> {
        let remote = format!("sh -c {}", ssh::shell_quote(script));
        let mut cmd = ssh::command(&self.target, &self.settings, KeyPolicy::Strict, &remote);
        if self.password {
            ssh::allow_password(&mut cmd);
        }
        cmd.stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
        let mut child = cmd.spawn().map_err(|e| SystemError::Io {
            path: "ssh".into(),
            source: e,
        })?;
        if let (Some(data), Some(mut stdin)) = (input, child.stdin.take()) {
            stdin.write_all(data).map_err(|e| SystemError::Io {
                path: "ssh stdin".into(),
                source: e,
            })?;
        }
        let out = crate::sync::capture(
            child,
            crate::sync::MAX_HELPER_STDOUT,
            crate::sync::MAX_HELPER_STDERR,
        )
        .map_err(|e| SystemError::Io {
            path: "ssh".into(),
            source: e,
        })?;
        if out.stdout_overflow {
            return Err(SystemError::InvalidState(
                "the helper's answer is larger than goway accepts".to_owned(),
            ));
        }
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_owned();
        if out.status.code() == Some(255) {
            return Err(SystemError::InvalidState(format!("ssh failed: {stderr}")));
        }
        Ok(Out {
            code: out.status.code(),
            stdout: out.stdout,
            stderr,
        })
    }

    fn check(&self, script: &str, what: &str, path: &Path) -> SysResult<()> {
        let out = self.exec(script, None)?;
        if out.code == Some(0) {
            Ok(())
        } else {
            Err(SystemError::InvalidState(format!(
                "remote {what} {} failed: {}",
                path.display(),
                out.stderr
            )))
        }
    }

    /// Run a snippet and return its trimmed stdout (identity checks, `$HOME`).
    pub fn output(&self, script: &str) -> SysResult<String> {
        let out = self.exec(script, None)?;
        if out.code == Some(0) {
            Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
        } else {
            Err(SystemError::InvalidState(out.stderr))
        }
    }
}

fn q(path: &Path) -> String {
    ssh::shell_quote(&path.to_string_lossy())
}

impl System for RemoteSystem {
    fn read_file(&self, path: &Path) -> SysResult<Option<String>> {
        let p = q(path);
        let out = self.exec(
            &format!("if [ -f {p} ]; then cat {p}; else exit 3; fi"),
            None,
        )?;
        match out.code {
            Some(0) => Ok(Some(String::from_utf8_lossy(&out.stdout).into_owned())),
            Some(3) => Ok(None),
            _ => Err(SystemError::InvalidState(out.stderr)),
        }
    }

    fn write_file(&mut self, path: &Path, contents: &str) -> SysResult<()> {
        tracing::info!(host = %self.target.name, path = %path.display(), "remote: write file");
        // Atomic (a dropped connection cannot leave a truncated
        // authorized_keys): write a temp file with the old mode, then rename.
        let p = q(path);
        let script = format!(
            "t={p}.goway-tmp.$$; cat > \"$t\" && {{ if [ -e {p} ]; then chmod --reference={p} \"$t\"; fi; }} && mv -f \"$t\" {p} || {{ rm -f \"$t\"; exit 1; }}"
        );
        let out = self.exec(&script, Some(contents.as_bytes()))?;
        if out.code == Some(0) {
            Ok(())
        } else {
            Err(SystemError::InvalidState(out.stderr))
        }
    }

    fn remove_file(&mut self, path: &Path) -> SysResult<()> {
        tracing::info!(host = %self.target.name, path = %path.display(), "remote: remove file");
        self.check(&format!("rm -f {}", q(path)), "remove", path)
    }

    fn dir_exists(&self, path: &Path) -> SysResult<bool> {
        Ok(self.exec(&format!("[ -d {} ]", q(path)), None)?.code == Some(0))
    }

    fn create_dir(&mut self, path: &Path) -> SysResult<()> {
        tracing::info!(host = %self.target.name, path = %path.display(), "remote: create dir");
        self.check(&format!("mkdir {}", q(path)), "mkdir", path)
    }

    fn dir_is_empty(&self, path: &Path) -> SysResult<bool> {
        let out = self.output(&format!("ls -A {} | head -1", q(path)))?;
        Ok(out.is_empty())
    }

    fn remove_dir(&mut self, path: &Path) -> SysResult<()> {
        tracing::info!(host = %self.target.name, path = %path.display(), "remote: remove dir");
        self.check(&format!("rmdir {}", q(path)), "rmdir", path)
    }

    fn get_var(&self, _: &str) -> SysResult<Option<String>> {
        Err(SystemError::Unsupported("variables on a remote host"))
    }

    fn set_var(&mut self, _: &str, _: &str) -> SysResult<()> {
        Err(SystemError::Unsupported("variables on a remote host"))
    }

    fn remove_var(&mut self, _: &str) -> SysResult<()> {
        Err(SystemError::Unsupported("variables on a remote host"))
    }

    fn file_digest(&self, path: &Path) -> SysResult<Option<String>> {
        let p = q(path);
        let out = self.output(&format!("if [ -f {p} ]; then sha256sum < {p}; fi"))?;
        Ok(out.split_whitespace().next().map(str::to_owned))
    }

    fn copy_file(&mut self, _: &Path, _: &Path) -> SysResult<()> {
        Err(SystemError::Unsupported("copying files to a remote host"))
    }

    fn reg_key_exists(&self, _: &str) -> SysResult<bool> {
        Err(SystemError::Unsupported("registry on a remote host"))
    }

    fn reg_key_create(&mut self, _: &str) -> SysResult<()> {
        Err(SystemError::Unsupported("registry on a remote host"))
    }

    fn reg_key_is_empty(&self, _: &str) -> SysResult<bool> {
        Err(SystemError::Unsupported("registry on a remote host"))
    }

    fn reg_key_remove(&mut self, _: &str) -> SysResult<()> {
        Err(SystemError::Unsupported("registry on a remote host"))
    }

    fn reg_get(&self, _: &str, _: &str) -> SysResult<Option<RegValue>> {
        Err(SystemError::Unsupported("registry on a remote host"))
    }

    fn reg_set(&mut self, _: &str, _: &str, _: &RegValue) -> SysResult<()> {
        Err(SystemError::Unsupported("registry on a remote host"))
    }

    fn reg_delete(&mut self, _: &str, _: &str) -> SysResult<()> {
        Err(SystemError::Unsupported("registry on a remote host"))
    }

    fn get_mode(&self, path: &Path) -> SysResult<u32> {
        let out = self.output(&format!("stat -c %a {}", q(path)))?;
        u32::from_str_radix(&out, 8).map_err(|_| {
            SystemError::InvalidState(format!("bad mode `{out}` for {}", path.display()))
        })
    }

    fn set_mode(&mut self, path: &Path, mode: u32) -> SysResult<()> {
        tracing::info!(host = %self.target.name, path = %path.display(), mode = format!("{mode:o}"), "remote: chmod");
        self.check(&format!("chmod {mode:o} {}", q(path)), "chmod", path)
    }

    fn get_acl(&self, _: &Path) -> SysResult<String> {
        Err(SystemError::Unsupported("ACLs on a remote host"))
    }

    fn set_acl(&mut self, _: &Path, _: &str) -> SysResult<()> {
        Err(SystemError::Unsupported("ACLs on a remote host"))
    }

    fn resource_exists(&self, _: ResourceKind, _: &str) -> SysResult<bool> {
        Err(SystemError::Unsupported("resources on a remote host"))
    }

    fn resource_create(&mut self, _: ResourceKind, _: &str, _: &str) -> SysResult<()> {
        Err(SystemError::Unsupported("resources on a remote host"))
    }

    fn resource_delete(&mut self, _: ResourceKind, _: &str) -> SysResult<()> {
        Err(SystemError::Unsupported("resources on a remote host"))
    }
}
