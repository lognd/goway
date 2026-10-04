//! `--with-git`: a `.git` for the helper's copy of the work tree.
//!
//! The copy goway sends has no `.git`, so tests that ask git about the
//! repository (`git status`, tracked paths, HEAD) fail there. This module
//! builds, on this machine, a minimal repository that answers like the real
//! one: the real HEAD (a shallow clone of exactly one commit: its tree and
//! blobs, no history) and the real index, so `git status` there lists the
//! same changes as here. It lives in goway's state directory, one per work
//! tree, and is rebuilt only when HEAD moves; its files then travel with the
//! rest of the tree as ordinary files (`.git/...`), so the sync, the copy
//! verification and gc treat them like any other.
//!
//! What is never in it: remotes, credentials, hooks, user config, reflogs,
//! stashes, other branches, history, or the committed content of
//! secret-looking files (the blobs of paths [`Secrets`] keeps local are left
//! out of the pack; git status does not need them). Staged content that is
//! not in HEAD is not included either: `git diff --cached` there cannot show
//! it, while `git status` can.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::config::Os;
use crate::error::{Error, Result};
use crate::repo::{self, Repo};
use crate::spawn::CommandExt as _;
use crate::sync::{Kind, LocalFile, Secrets};

/// A built overlay: the directory holding `.git/` and the files in it.
#[derive(Debug, Clone)]
pub struct Overlay {
    /// Directory that contains `.git/` (the files sent are `.git/...` in it).
    pub root: PathBuf,
    /// The `.git/...` files, as sync entries.
    pub files: Vec<LocalFile>,
}

/// Bumped when what goes into the pack changes, to invalidate cached packs.
const LAYOUT: &str = "1";

fn git_failed(what: &str, detail: impl std::fmt::Display) -> Error {
    Error::Git {
        message: format!("--with-git: {what}: {detail}"),
    }
}

/// Write `bytes` to `path` unless it already holds exactly them (keeps mtimes
/// stable, so an unchanged file is never sent again).
fn write_if_changed(path: &Path, bytes: &[u8]) -> Result<()> {
    if std::fs::read(path).is_ok_and(|old| old == bytes) {
        return Ok(());
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| Error::io("create", dir, e))?;
    }
    std::fs::write(path, bytes).map_err(|e| Error::io("write", path, e))
}

/// The repository config the helper gets: only what makes git read the copy
/// as this one does (no remotes, no credentials, no hooks path).
fn config_text(root: &Path, host_os: Os) -> String {
    use std::fmt::Write as _;
    let windows = host_os == Os::Windows;
    let mut text = String::from("[core]\n\trepositoryformatversion = 0\n\tbare = false\n");
    let _ = writeln!(text, "\tfilemode = {}", !windows);
    if windows {
        text.push_str("\tsymlinks = false\n\tignorecase = true\n");
    }
    for key in ["core.autocrlf", "core.eol", "core.safecrlf"] {
        let value = repo::git(root, &["config", "--get", key])
            .ok()
            .filter(|v| v.chars().all(|c| c.is_ascii_alphanumeric()));
        if let Some(v) = value {
            let _ = writeln!(text, "\t{} = {v}", key.trim_start_matches("core."));
        }
    }
    text
}

/// Run `cmd` with stdin and stdout taken from files, failing with git's stderr.
fn run_files(mut cmd: Command, stdin: &Path, stdout: Option<&Path>, what: &str) -> Result<()> {
    let input = std::fs::File::open(stdin).map_err(|e| Error::io("open", stdin, e))?;
    cmd.stdin(Stdio::from(input)).stderr(Stdio::piped());
    match stdout {
        Some(p) => {
            let f = std::fs::File::create(p).map_err(|e| Error::io("create", p, e))?;
            cmd.stdout(Stdio::from(f));
        }
        None => {
            cmd.stdout(Stdio::null());
        }
    }
    let out = cmd
        .spawn_locked()
        .and_then(std::process::Child::wait_with_output)
        .map_err(|e| git_failed(what, e))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(git_failed(
            what,
            String::from_utf8_lossy(&out.stderr).trim(),
        ))
    }
}

/// Pack the objects of HEAD's commit (its tree and blobs, minus secret-looking
/// paths) into `dir`'s `.git/objects/pack`.
fn build_pack(root: &Path, dir: &Path, secrets: &Secrets) -> Result<()> {
    let gitdir = dir.join(".git");
    let pack_dir = gitdir.join("objects").join("pack");
    if pack_dir.is_dir() {
        std::fs::remove_dir_all(&pack_dir).map_err(|e| Error::io("remove", &pack_dir, e))?;
    }
    std::fs::create_dir_all(&pack_dir).map_err(|e| Error::io("create", &pack_dir, e))?;
    let listed = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["rev-list", "--objects", "--max-count=1", "HEAD"])
        .stdin(Stdio::null())
        .output_locked()
        .map_err(|e| git_failed("cannot run git", e))?;
    if !listed.status.success() {
        return Err(git_failed(
            "rev-list",
            String::from_utf8_lossy(&listed.stderr).trim(),
        ));
    }
    let mut wanted = Vec::new();
    let mut left_out = 0usize;
    for line in listed
        .stdout
        .split(|b| *b == b'\n')
        .filter(|l| !l.is_empty())
    {
        let line = String::from_utf8_lossy(line);
        let (oid, path) = line.split_once(' ').unwrap_or((line.as_ref(), ""));
        if !path.is_empty() && secrets.keeps_local(path) {
            left_out += 1;
            tracing::debug!(path, "secret-looking path left out of the helper's .git");
            continue;
        }
        wanted.extend_from_slice(oid.as_bytes());
        wanted.push(b'\n');
    }
    let list = dir.join("objects.list");
    let pack = dir.join("head.pack");
    std::fs::write(&list, &wanted).map_err(|e| Error::io("write", &list, e))?;
    let mut pack_objects = Command::new("git");
    pack_objects
        .arg("-C")
        .arg(root)
        .args(["pack-objects", "--stdout", "-q"]);
    run_files(pack_objects, &list, Some(&pack), "pack-objects")?;
    let mut index_pack = Command::new("git");
    index_pack
        .env("GIT_DIR", &gitdir)
        .args(["index-pack", "--stdin"]);
    run_files(index_pack, &pack, None, "index-pack")?;
    let _ = std::fs::remove_file(&list);
    let _ = std::fs::remove_file(&pack);
    tracing::info!(left_out, "built the helper's HEAD pack");
    Ok(())
}

/// Every file under `dir/.git`, as sync entries.
fn list_files(dir: &Path) -> Result<Vec<LocalFile>> {
    let mut out = Vec::new();
    let mut stack = vec![dir.join(".git")];
    while let Some(d) = stack.pop() {
        for entry in std::fs::read_dir(&d).map_err(|e| Error::io("read", &d, e))? {
            let entry = entry.map_err(|e| Error::io("read", &d, e))?;
            let path = entry.path();
            let meta = entry.metadata().map_err(|e| Error::io("stat", &path, e))?;
            if meta.is_dir() {
                stack.push(path);
            } else if meta.is_file() {
                let rel = path.strip_prefix(dir).map_or_else(
                    |_| String::new(),
                    |p| p.to_string_lossy().replace('\\', "/"),
                );
                let mtime = meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map_or(0, |d| d.as_secs());
                out.push(LocalFile {
                    path: rel,
                    size: meta.len(),
                    mtime,
                    kind: Kind::File { exec: false },
                });
            }
        }
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(out)
}

/// Build (or refresh) the overlay for `repo`'s work tree under `state_dir`,
/// for a helper running `host_os`.
///
/// # Errors
///
/// [`Error::Git`] when git fails or the repository has no commit yet (nothing
/// to copy), [`Error::Io`] when the overlay cannot be written.
pub fn prepare(repo: &Repo, secrets: &Secrets, host_os: Os, state_dir: &Path) -> Result<Overlay> {
    let root = repo.root.as_path();
    let head = repo::git(root, &["rev-parse", "--verify", "-q", "HEAD^{commit}"])
        .map_err(|_| git_failed("this repository has no commit yet", "nothing to copy"))?;
    if head.len() != 40 || !head.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(git_failed("unsupported object format", &head));
    }
    let branch = repo::git(root, &["symbolic-ref", "-q", "HEAD"]).ok();
    let dir = state_dir.join("git-meta").join(&repo.worktree_id);
    let gitdir = dir.join(".git");
    let marker = dir.join("head");
    let want = format!("{LAYOUT} {head}\n");
    let built = std::fs::read_to_string(&marker).is_ok_and(|m| m == want) && gitdir.is_dir();
    if !built {
        tracing::info!(head, "building the helper's .git");
        if gitdir.exists() {
            std::fs::remove_dir_all(&gitdir).map_err(|e| Error::io("remove", &gitdir, e))?;
        }
        std::fs::create_dir_all(&dir).map_err(|e| Error::io("create", &dir, e))?;
        let init = Command::new("git")
            .arg("init")
            .arg("-q")
            .arg("--template=")
            .arg(&dir)
            .stdin(Stdio::null())
            .output_locked()
            .map_err(|e| git_failed("cannot run git", e))?;
        if !init.status.success() {
            return Err(git_failed(
                "init",
                String::from_utf8_lossy(&init.stderr).trim(),
            ));
        }
        build_pack(root, &dir, secrets)?;
        write_if_changed(&gitdir.join("shallow"), format!("{head}\n").as_bytes())?;
        // A ref file always exists, or git would not see a repository (the
        // tar carries files, not empty directories).
        let (head_text, ref_path) = match &branch {
            Some(name) => (format!("ref: {name}\n"), gitdir.join(name)),
            None => (format!("{head}\n"), gitdir.join("refs/goway/head")),
        };
        write_if_changed(&gitdir.join("HEAD"), head_text.as_bytes())?;
        write_if_changed(&ref_path, format!("{head}\n").as_bytes())?;
        std::fs::write(&marker, &want).map_err(|e| Error::io("write", &marker, e))?;
    }
    write_if_changed(
        &gitdir.join("config"),
        config_text(root, host_os).as_bytes(),
    )?;
    let index = repo::git(root, &["rev-parse", "--git-path", "index"])?;
    let index = if Path::new(&index).is_absolute() {
        PathBuf::from(index)
    } else {
        root.join(index)
    };
    match std::fs::read(&index) {
        Ok(bytes) => write_if_changed(&gitdir.join("index"), &bytes)?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let _ = std::fs::remove_file(gitdir.join("index"));
        }
        Err(e) => return Err(Error::io("read", &index, e)),
    }
    let files = list_files(&dir)?;
    tracing::info!(files = files.len(), "helper .git ready");
    Ok(Overlay { root: dir, files })
}
