//! Identity of the local repository and work tree, from git.
//!
//! The repository id is stable across clones and worktrees (hash of the
//! root commit), so every worktree of one repository shares remote caches;
//! the worktree id is unique per client machine and path, so concurrent
//! runs from different worktrees never share a seed.

use std::path::{Path, PathBuf};
use std::process::Command;

use sha2::{Digest as _, Sha256};

use crate::error::{Error, Result};
use crate::spawn::CommandExt as _;

/// The local repository and work tree a run comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repo {
    /// Top of the work tree.
    pub root: PathBuf,
    /// Readable repository name (directory of the main worktree).
    pub name: String,
    /// Stable repository id (hex), shared by all worktrees and clones.
    pub id: String,
    /// Id of this work tree on this client (hex).
    pub worktree_id: String,
    /// This client's host name, for labels.
    pub client: String,
}

/// The null device as git spells it for `core.hooksPath`.
const NO_HOOKS: &str = if cfg!(windows) { "NUL" } else { "/dev/null" };

/// A `git` command for a repository goway does not trust: the repository's
/// own config cannot start a program through `core.fsmonitor` or
/// `core.hooksPath`, no pager or editor is used, and the user's and
/// system's global hooks directory is replaced by nothing. Every goway git
/// call starts here. (`ls-files`, `rev-list` and `pack-objects` apply no
/// clean/smudge filter or diff driver, so those need no further switch.)
pub fn git_command() -> Command {
    let mut c = Command::new("git");
    c.arg("-c")
        .arg("core.fsmonitor=false")
        .arg("-c")
        .arg(format!("core.hooksPath={NO_HOOKS}"))
        .arg("-c")
        .arg("core.pager=cat")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env_remove("GIT_EXTERNAL_DIFF");
    c
}

/// Run git in `dir` and return trimmed stdout.
pub fn git(dir: &Path, args: &[&str]) -> Result<String> {
    let out = git_command()
        .arg("-C")
        .arg(dir)
        .args(args)
        .stdin(std::process::Stdio::null())
        .output_locked()
        .map_err(|e| Error::Git {
            message: format!("cannot run git: {e}"),
        })?;
    if !out.status.success() {
        return Err(Error::Git {
            message: format!(
                "git {} failed: {}",
                args.join(" "),
                String::from_utf8_lossy(&out.stderr).trim()
            ),
        });
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

/// Create a repository in `dir` whose first branch is `main`. Works on git
/// older than 2.28, which has no `git init -b`; tests use it everywhere.
pub fn init_main(dir: &Path) -> Result<()> {
    git(dir, &["init", "-q"])?;
    git(dir, &["symbolic-ref", "HEAD", "refs/heads/main"])?;
    Ok(())
}

/// Short hex digest of `parts`, joined with NUL.
pub fn short_hash(parts: &[&str]) -> String {
    let mut h = Sha256::new();
    for p in parts {
        h.update(p.as_bytes());
        h.update([0]);
    }
    let digest = h.finalize();
    digest[..8].iter().fold(String::new(), |mut s, b| {
        use std::fmt::Write as _;
        let _ = write!(s, "{b:02x}");
        s
    })
}

/// This machine's host name (best effort; labels only).
pub fn client_name() -> String {
    std::env::var("HOSTNAME")
        .ok()
        .or_else(|| std::env::var("COMPUTERNAME").ok())
        .or_else(|| {
            std::fs::read_to_string("/etc/hostname")
                .ok()
                .map(|s| s.trim().to_owned())
        })
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_owned())
}

impl Repo {
    /// Discover the repository containing `dir`.
    pub fn discover(dir: &Path) -> Result<Self> {
        let root = git(dir, &["rev-parse", "--show-toplevel"]).map_err(|e| match e {
            Error::Git { message } if message.contains("not a git repository") => Error::Usage(format!(
                "goway runs a command from inside a git project, and {} is not in one\n  next: `cd` into your project folder first (or make this folder a project with `git init`)",
                dir.display()
            )),
            other => other,
        })?;
        let root = PathBuf::from(root);
        // `--path-format=absolute` needs git 2.31; older gits answer a path
        // relative to `root` (or absolute), which `join` resolves either way.
        let common = root.join(git(&root, &["rev-parse", "--git-common-dir"])?);
        let main_tree = if common.file_name().is_some_and(|n| n == ".git") {
            common
                .parent()
                .map_or_else(|| root.clone(), Path::to_path_buf)
        } else {
            common.clone()
        };
        let name = main_tree
            .file_name()
            .map_or_else(|| "repo".to_owned(), |n| n.to_string_lossy().into_owned());
        let roots = git(&root, &["rev-list", "--max-parents=0", "HEAD"]).unwrap_or_default();
        let first_root = roots.lines().last().unwrap_or("").to_owned();
        // Worktrees of one clone share caches; a fork or another clone with
        // the same history but a different origin gets its own, so code
        // from one cannot poison the build artifacts of the other.
        let origin = git(&root, &["config", "--get", "remote.origin.url"]).unwrap_or_default();
        let id = if first_root.is_empty() {
            short_hash(&["path", &common.to_string_lossy()])
        } else if origin.is_empty() {
            short_hash(&["root", &first_root, "clone", &common.to_string_lossy()])
        } else {
            short_hash(&["root", &first_root, "origin", &origin])
        };
        let client = client_name();
        let worktree_id = short_hash(&[&client, &root.to_string_lossy()]);
        let repo = Self {
            root,
            name,
            id,
            worktree_id,
            client,
        };
        tracing::debug!(?repo, "repository");
        Ok(repo)
    }

    /// The seed's path under the remote root: `<repo-id>/<worktree-id>`.
    pub fn seed_key(&self) -> String {
        format!("{}/{}", self.id, self.worktree_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn init_repo(dir: &Path) {
        init_main(dir).unwrap();
        for args in [
            vec!["config", "user.email", "t@example.com"],
            vec!["config", "user.name", "t"],
            vec!["config", "core.autocrlf", "false"],
        ] {
            git(dir, &args).unwrap();
        }
    }

    // frob:tests crates/goway/src/repo.rs::client_name
    #[test]
    fn ids_are_stable_across_worktrees_and_distinct_per_tree() {
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("proj");
        std::fs::create_dir(&main).unwrap();
        init_repo(&main);
        let before = Repo::discover(&main).unwrap();
        std::fs::write(main.join("a"), "a").unwrap();
        git(&main, &["add", "a"]).unwrap();
        git(&main, &["commit", "-qm", "a"]).unwrap();
        let r1 = Repo::discover(&main).unwrap();
        assert_ne!(before.id, r1.id, "id moves from path to root commit");
        let wt = dir.path().join("wt");
        git(&main, &["worktree", "add", "-q", wt.to_str().unwrap()]).unwrap();
        let r2 = Repo::discover(&wt).unwrap();
        assert_eq!(r1.id, r2.id);
        assert_eq!(r2.name, "proj");
        assert_ne!(r1.worktree_id, r2.worktree_id);
        assert_eq!(r1.seed_key(), format!("{}/{}", r1.id, r1.worktree_id));
        assert!(Repo::discover(dir.path()).is_err());

        // A fork: same root commit, different origin -> separate caches.
        let fork = dir.path().join("fork");
        git(
            dir.path(),
            &[
                "clone",
                "-q",
                main.to_str().unwrap(),
                fork.to_str().unwrap(),
            ],
        )
        .unwrap();
        let upstream = dir.path().join("upstream");
        git(
            dir.path(),
            &[
                "clone",
                "-q",
                main.to_str().unwrap(),
                upstream.to_str().unwrap(),
            ],
        )
        .unwrap();
        git(
            &fork,
            &[
                "remote",
                "set-url",
                "origin",
                "https://example.com/someone-else/proj",
            ],
        )
        .unwrap();
        let f = Repo::discover(&fork).unwrap();
        let u = Repo::discover(&upstream).unwrap();
        assert_ne!(f.id, u.id, "forks with the same root commit share no cache");
    }
}
