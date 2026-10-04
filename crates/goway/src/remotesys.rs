//! A journal `System` whose files live on a remote host, reached over ssh.
//!
//! Used by `goway ssh setup` so the changes it makes on the host
//! (`~/.ssh`, `authorized_keys`, their modes) are journaled and undone the
//! same way the installers' local changes are. Only file, directory and
//! mode operations are supported; files are replaced atomically and keep
//! their mode.

use std::fmt::Write as _;
use std::io::Write as _;
use std::path::Path;
use std::process::Stdio;

use goway_journal::{RegValue, ResourceKind, SysResult, System, SystemError};

use crate::spawn::CommandExt as _;
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
    /// How the password prompt names the host (`None`: ssh's own alias).
    pub prompt: Option<ssh::PasswordPrompt>,
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
            ssh::allow_password(&mut cmd, self.prompt.as_ref());
        }
        cmd.stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
        let mut child = cmd.spawn_locked().map_err(|e| SystemError::Io {
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

    fn resource_exists(&self, kind: ResourceKind, name: &str) -> SysResult<bool> {
        let script = match kind {
            ResourceKind::PinnedTool => pinned_exists(&PinName::parse(name)?),
            ResourceKind::RustupTarget => rustup_exists(&RustupName::parse(name)?),
            _ => return Err(SystemError::Unsupported("these resources on a remote host")),
        };
        Ok(self.exec(&script, None)?.code == Some(0))
    }

    fn resource_create(&mut self, kind: ResourceKind, name: &str, spec: &str) -> SysResult<()> {
        let (script, what) = match kind {
            ResourceKind::PinnedTool => {
                PinName::parse(name)?;
                (spec.to_owned(), "install a pinned tool")
            }
            ResourceKind::RustupTarget => (
                rustup_run("add", &RustupName::parse(name)?),
                "add a rustup target",
            ),
            _ => return Err(SystemError::Unsupported("these resources on a remote host")),
        };
        self.check(&script, what, Path::new(name))
    }

    fn resource_delete(&mut self, kind: ResourceKind, name: &str) -> SysResult<()> {
        let (script, what) = match kind {
            ResourceKind::PinnedTool => (
                pinned_delete(&PinName::parse(name)?),
                "remove a pinned tool",
            ),
            ResourceKind::RustupTarget => (
                rustup_run("remove", &RustupName::parse(name)?),
                "remove a rustup target",
            ),
            _ => return Err(SystemError::Unsupported("these resources on a remote host")),
        };
        self.check(&script, what, Path::new(name))
    }

    fn resource_outdated(&self, kind: ResourceKind, name: &str) -> SysResult<Option<String>> {
        if kind != ResourceKind::PinnedTool {
            return Ok(None);
        }
        let pin = PinName::parse(name)?;
        if !self.resource_exists(kind, name)? {
            return Ok(None);
        }
        let out = self.exec(&pinned_snapshot(&pin), None)?;
        if out.code == Some(0) {
            Ok(Some(String::from_utf8_lossy(&out.stdout).into_owned()))
        } else {
            Err(SystemError::InvalidState(out.stderr))
        }
    }

    fn resource_restore(
        &mut self,
        kind: ResourceKind,
        name: &str,
        snapshot: &str,
    ) -> SysResult<()> {
        if kind != ResourceKind::PinnedTool {
            return Err(SystemError::Unsupported("restoring these resources"));
        }
        let pin = PinName::parse(name)?;
        if snapshot.lines().any(|l| l == "D") {
            tracing::warn!(tool = %pin.tool, "an earlier tree of the tool was replaced; it is not retained, only its outside links are restored");
        }
        let mut script = String::from("mkdir -p \"$HOME/.local/bin\"\n");
        for line in snapshot.lines() {
            let Some(rest) = line.strip_prefix("L ") else {
                continue;
            };
            let (link, target) = rest.split_once(' ').ok_or_else(|| {
                SystemError::InvalidState(format!("malformed link snapshot line {line:?}"))
            })?;
            if !pin.links.iter().any(|l| l == link) {
                return Err(SystemError::InvalidState(format!(
                    "snapshot names a link {link:?} the tool does not own"
                )));
            }
            let _ = writeln!(
                script,
                "ln -sfn {} \"$HOME/.local/bin/{link}\"",
                ssh::shell_quote(target)
            );
        }
        self.check(&script, "restore replaced links of", Path::new(name))
    }
}

/// The name of a [`ResourceKind::PinnedTool`]: `TOOL:link1,link2`.
struct PinName {
    tool: String,
    links: Vec<String>,
}

fn plain(text: &str, extra: &[char]) -> bool {
    !text.is_empty()
        && !text.starts_with('-')
        && text.chars().all(|c| {
            c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') || extra.contains(&c)
        })
}

impl PinName {
    fn parse(name: &str) -> SysResult<Self> {
        let bad = || SystemError::InvalidState(format!("not a pinned tool name: {name:?}"));
        let (tool, links) = name.split_once(':').ok_or_else(bad)?;
        let links: Vec<String> = links.split(',').map(str::to_owned).collect();
        if !plain(tool, &[]) || !links.iter().all(|l| plain(l, &[])) {
            return Err(bad());
        }
        Ok(Self {
            tool: tool.to_owned(),
            links,
        })
    }

    fn dir(&self) -> String {
        format!("\"$HOME/.local/opt/goway-{}\"", self.tool)
    }
}

/// Exit 0 when the tool's tree, or any of its link names, is there.
fn pinned_exists(pin: &PinName) -> String {
    let mut script = format!("[ -d {} ] && exit 0\n", pin.dir());
    for l in &pin.links {
        let _ = writeln!(
            script,
            "{{ [ -L \"$HOME/.local/bin/{l}\" ] || [ -e \"$HOME/.local/bin/{l}\" ]; }} && exit 0"
        );
    }
    script.push_str("exit 1\n");
    script
}

/// Print `L name target` for each outside link the install would replace and `D` when a tree is
/// there; refuse (exit 3) to replace a regular file goway did not make.
fn pinned_snapshot(pin: &PinName) -> String {
    let dir = pin.dir();
    let mut script = String::new();
    for l in &pin.links {
        let _ = write!(
            script,
            "p=\"$HOME/.local/bin/{l}\"\nif [ -L \"$p\" ]; then t=$(readlink \"$p\"); case \"$t\" in {dir}/*) ;; *) printf 'L %s %s\\n' '{l}' \"$t\" ;; esac\nelif [ -e \"$p\" ]; then echo \"goway: $p is a file goway did not make; not replacing it\" >&2; exit 3; fi\n"
        );
    }
    let _ = write!(script, "[ -d {dir} ] && echo D\nexit 0\n");
    script
}

/// Remove the links into the tool's tree, the tree, and the parents when they end up empty.
fn pinned_delete(pin: &PinName) -> String {
    let dir = pin.dir();
    format!(
        "for l in \"$HOME\"/.local/bin/*; do case \"$(readlink \"$l\")\" in {dir}/*) rm -f \"$l\" ;; esac; done; rm -rf {dir}; rmdir \"$HOME/.local/opt\" \"$HOME/.local/bin\" 2>/dev/null; true\n"
    )
}

/// The name of a [`ResourceKind::RustupTarget`]: `TARGET` or `TOOLCHAIN/TARGET`.
struct RustupName {
    toolchain: Option<String>,
    target: String,
}

impl RustupName {
    fn parse(name: &str) -> SysResult<Self> {
        let bad = || SystemError::InvalidState(format!("not a rustup target name: {name:?}"));
        let (toolchain, target) = match name.split_once('/') {
            Some((t, g)) => (Some(t), g),
            None => (None, name),
        };
        if !plain(target, &[]) || toolchain.is_some_and(|t| !plain(t, &[])) {
            return Err(bad());
        }
        Ok(Self {
            toolchain: toolchain.map(str::to_owned),
            target: target.to_owned(),
        })
    }

    fn flag(&self) -> String {
        self.toolchain
            .as_ref()
            .map_or_else(String::new, |t| format!(" --toolchain {t}"))
    }
}

const RUSTUP_PATH: &str = "PATH=\"${CARGO_HOME:-$HOME/.cargo}/bin:$PATH\"; ";

fn rustup_exists(r: &RustupName) -> String {
    format!(
        "{RUSTUP_PATH}rustup target list --installed{} 2>/dev/null | grep -qx '{}'\n",
        r.flag(),
        r.target
    )
}

fn rustup_run(verb: &str, r: &RustupName) -> String {
    format!(
        "{RUSTUP_PATH}rustup target {verb}{} {}\n",
        r.flag(),
        r.target
    )
}
