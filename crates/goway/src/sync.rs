//! Syncing the work tree to the remote seed mirror.
//!
//! The file set is what the user sees in git: tracked plus untracked files
//! that are not ignored (`git ls-files -co --exclude-standard`), minus files
//! deleted from the work tree and `.env` files. The remote reports its seed
//! manifest; goway sends a tar of what differs (size, mtime seconds, exec
//! bit, symlink target) and a list of deletions. Mtimes are preserved, which
//! is what keeps cargo's fingerprints fresh on the remote.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use base64::Engine as _;

use crate::error::{Error, Result};
use crate::remote;
use crate::repo::{self, Repo};
use crate::ssh::{self, KeyPolicy, Target};

/// What a local entry is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    /// A regular file; `exec` is the owner execute bit.
    File {
        /// Whether the file is executable.
        exec: bool,
    },
    /// A symbolic link to `target`.
    Symlink {
        /// The link target, as stored.
        target: String,
    },
}

/// One file of the local work tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalFile {
    /// Path relative to the work tree root, `/`-separated.
    pub path: String,
    /// Size in bytes (0 for symlinks).
    pub size: u64,
    /// Modification time, whole seconds since the epoch.
    pub mtime: u64,
    /// File or symlink.
    pub kind: Kind,
}

/// One entry of the remote seed manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteEntry {
    /// Size in bytes.
    pub size: u64,
    /// Modification time, whole seconds.
    pub mtime: u64,
    /// File or symlink (with exec bit / target).
    pub kind: Kind,
}

/// Whether `path` is an env file goway never sends by default.
pub fn is_env_file(path: &str) -> bool {
    let base = path.rsplit('/').next().unwrap_or(path);
    base == ".env" || base.starts_with(".env.")
}

/// The work tree's file set, sorted by path.
pub fn file_set(root: &Path, send_env_files: bool) -> Result<Vec<LocalFile>> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["ls-files", "-co", "--exclude-standard", "-z", "--full-name"])
        .output()
        .map_err(|e| Error::Git {
            message: format!("cannot run git: {e}"),
        })?;
    if !out.status.success() {
        return Err(Error::Git {
            message: String::from_utf8_lossy(&out.stderr).trim().to_owned(),
        });
    }
    let modes = index_exec_bits(root)?;
    let mut seen = BTreeSet::new();
    let mut files = Vec::new();
    let mut skipped_env = 0usize;
    for raw in out.stdout.split(|b| *b == 0).filter(|p| !p.is_empty()) {
        let path = String::from_utf8_lossy(raw).into_owned();
        if !seen.insert(path.clone()) {
            continue;
        }
        if !send_env_files && is_env_file(&path) {
            skipped_env += 1;
            tracing::debug!(path, "env file not sent");
            continue;
        }
        let full = root.join(&path);
        let Ok(meta) = std::fs::symlink_metadata(&full) else {
            tracing::trace!(path, "deleted in work tree; not sent");
            continue;
        };
        let mtime = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_secs());
        let kind = if meta.file_type().is_symlink() {
            let target = std::fs::read_link(&full).map_err(|e| Error::io("read link", &full, e))?;
            Kind::Symlink {
                target: target.to_string_lossy().replace('\\', "/"),
            }
        } else if meta.is_file() {
            Kind::File {
                exec: exec_bit(&meta, modes.get(&path).copied()),
            }
        } else {
            // Submodules and other directories listed by git.
            tracing::debug!(path, "not a file; skipped");
            continue;
        };
        let size = if matches!(kind, Kind::File { .. }) {
            meta.len()
        } else {
            0
        };
        files.push(LocalFile {
            path,
            size,
            mtime,
            kind,
        });
    }
    if skipped_env > 0 {
        tracing::info!(skipped_env, "env files kept local");
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(files)
}

/// Exec bits from the git index (used where the OS has none, i.e. Windows).
fn index_exec_bits(root: &Path) -> Result<BTreeMap<String, bool>> {
    if cfg!(unix) {
        return Ok(BTreeMap::new());
    }
    let text = repo::git(root, &["ls-files", "-s", "--full-name"])?;
    Ok(text
        .lines()
        .filter_map(|l| {
            let (meta, path) = l.split_once('\t')?;
            Some((path.to_owned(), meta.starts_with("100755")))
        })
        .collect())
}

#[cfg(unix)]
fn exec_bit(meta: &std::fs::Metadata, _: Option<bool>) -> bool {
    use std::os::unix::fs::PermissionsExt as _;
    meta.permissions().mode() & 0o100 != 0
}

#[cfg(not(unix))]
fn exec_bit(_: &std::fs::Metadata, from_index: Option<bool>) -> bool {
    from_index.unwrap_or(false)
}

/// Parse the NUL-terminated manifest the remote `manifest` verb prints.
pub fn parse_manifest(bytes: &[u8]) -> BTreeMap<String, RemoteEntry> {
    let mut out = BTreeMap::new();
    for rec in bytes.split(|b| *b == 0).filter(|r| !r.is_empty()) {
        let rec = String::from_utf8_lossy(rec);
        let mut f = rec.splitn(6, '\t');
        let (Some(ty), Some(size), Some(mtime), Some(mode), Some(link), Some(path)) =
            (f.next(), f.next(), f.next(), f.next(), f.next(), f.next())
        else {
            tracing::warn!(record = %rec, "malformed manifest record");
            continue;
        };
        let mtime = mtime
            .split('.')
            .next()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let kind = if ty == "l" {
            Kind::Symlink {
                target: link.to_owned(),
            }
        } else {
            let mode = u32::from_str_radix(mode, 8).unwrap_or(0);
            Kind::File {
                exec: mode & 0o100 != 0,
            }
        };
        out.insert(
            path.to_owned(),
            RemoteEntry {
                size: size.parse().unwrap_or(0),
                mtime,
                kind,
            },
        );
    }
    out
}

/// What to send and what to delete.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Plan {
    /// Indices into the local file list of entries to send.
    pub send: Vec<usize>,
    /// Remote paths to delete.
    pub delete: Vec<String>,
}

/// Compare the local file set with the remote manifest.
pub fn diff(local: &[LocalFile], remote: &BTreeMap<String, RemoteEntry>) -> Plan {
    let mut plan = Plan::default();
    let mut local_paths = BTreeSet::new();
    for (i, f) in local.iter().enumerate() {
        local_paths.insert(f.path.as_str());
        let same = remote
            .get(&f.path)
            .is_some_and(|r| match (&f.kind, &r.kind) {
                (Kind::Symlink { target: a }, Kind::Symlink { target: b }) => a == b,
                (Kind::File { exec: a }, Kind::File { exec: b }) => {
                    a == b && f.size == r.size && f.mtime == r.mtime
                }
                _ => false,
            });
        if !same {
            plan.send.push(i);
        }
    }
    plan.delete = remote
        .keys()
        .filter(|p| !local_paths.contains(p.as_str()))
        .cloned()
        .collect();
    plan
}

/// Write the selected files as a tar stream, preserving mtime and exec bit.
pub fn write_tar<W: std::io::Write>(root: &Path, files: &[&LocalFile], out: W) -> Result<W> {
    let mut builder = tar::Builder::new(out);
    builder.mode(tar::HeaderMode::Deterministic);
    for f in files {
        let mut header = tar::Header::new_gnu();
        header.set_mtime(f.mtime);
        header.set_uid(0);
        header.set_gid(0);
        match &f.kind {
            Kind::Symlink { target } => {
                header.set_entry_type(tar::EntryType::Symlink);
                header.set_size(0);
                header.set_mode(0o777);
                builder
                    .append_link(&mut header, &f.path, target)
                    .map_err(|e| Error::io("archive", root.join(&f.path), e))?;
            }
            Kind::File { exec } => {
                let full = root.join(&f.path);
                let file = std::fs::File::open(&full).map_err(|e| Error::io("open", &full, e))?;
                let len = file
                    .metadata()
                    .map_err(|e| Error::io("stat", &full, e))?
                    .len();
                header.set_entry_type(tar::EntryType::Regular);
                header.set_size(len);
                header.set_mode(if *exec { 0o755 } else { 0o644 });
                builder
                    .append_data(&mut header, &f.path, file)
                    .map_err(|e| Error::io("archive", &full, e))?;
            }
        }
    }
    builder
        .into_inner()
        .map_err(|e| Error::io("finish archive", PathBuf::from("<tar>"), e))
}

/// Label written next to every seed (and later work dir and cache).
#[derive(Debug, Clone, serde::Serialize)]
pub struct Label<'a> {
    /// What the directory is: seed, work or cache.
    pub kind: &'a str,
    /// Repository name.
    pub repo: &'a str,
    /// Repository id.
    pub repo_id: &'a str,
    /// Local work tree path.
    pub worktree: String,
    /// Client host name.
    pub client: &'a str,
    /// Seconds since the epoch.
    pub updated: u64,
}

/// Counts of one sync, for the user and the logs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Stats {
    /// Files in the local set.
    pub files: usize,
    /// Files sent.
    pub sent: usize,
    /// Bytes of file content sent.
    pub bytes: u64,
    /// Remote files deleted.
    pub deleted: usize,
}

/// Runs remote script invocations; ssh in production, a local shell in tests.
pub trait Transport {
    /// Run `cmd` and return its stdout.
    fn output(&self, cmd: &str) -> Result<Vec<u8>>;
    /// Run `cmd`, streaming what `feed` writes into its stdin.
    fn feed(
        &self,
        cmd: &str,
        feed: &mut dyn FnMut(&mut dyn std::io::Write) -> Result<()>,
    ) -> Result<()>;
}

/// The production transport: the system ssh to a resolved target.
#[derive(Debug, Clone)]
pub struct SshTransport<'a> {
    /// Where to run.
    pub target: &'a Target,
    /// ssh settings.
    pub settings: &'a ssh::Settings,
}

impl SshTransport<'_> {
    fn fail(&self, message: String) -> Error {
        Error::Ssh {
            host: self.target.name.clone(),
            message,
        }
    }
}

impl Transport for SshTransport<'_> {
    fn output(&self, cmd: &str) -> Result<Vec<u8>> {
        let out = ssh::command(self.target, self.settings, KeyPolicy::Strict, cmd)
            .stdin(Stdio::null())
            .output()
            .map_err(|e| self.fail(format!("cannot run ssh: {e}")))?;
        if !out.status.success() {
            return Err(self.fail(String::from_utf8_lossy(&out.stderr).trim().to_owned()));
        }
        Ok(out.stdout)
    }

    fn feed(
        &self,
        cmd: &str,
        feed: &mut dyn FnMut(&mut dyn std::io::Write) -> Result<()>,
    ) -> Result<()> {
        feed_child(
            ssh::command(self.target, self.settings, KeyPolicy::Strict, cmd),
            feed,
        )
        .map_err(|e| match e {
            Error::Ssh { message, .. } => self.fail(message),
            other => other,
        })
    }
}

/// Spawn `cmd`, write its stdin with `feed`, and require success.
pub fn feed_child(
    mut cmd: std::process::Command,
    feed: &mut dyn FnMut(&mut dyn std::io::Write) -> Result<()>,
) -> Result<()> {
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| Error::Ssh {
            host: String::new(),
            message: format!("cannot spawn: {e}"),
        })?;
    let written = child.stdin.take().map(|stdin| {
        let mut w = std::io::BufWriter::new(stdin);
        feed(&mut w).and_then(|()| {
            w.flush()
                .map_err(|e| Error::io("send", PathBuf::from("<remote>"), e))
        })
    });
    let out = child.wait_with_output().map_err(|e| Error::Ssh {
        host: String::new(),
        message: format!("wait failed: {e}"),
    })?;
    if !out.status.success() {
        return Err(Error::Ssh {
            host: String::new(),
            message: String::from_utf8_lossy(&out.stderr).trim().to_owned(),
        });
    }
    if let Some(Err(e)) = written {
        return Err(e);
    }
    Ok(())
}

/// Bring the remote seed of `repo` up to date with the local work tree.
pub fn sync(
    transport: &dyn Transport,
    remote_root: &str,
    repo: &Repo,
    send_env_files: bool,
) -> Result<Stats> {
    let started = std::time::Instant::now();
    let local = file_set(&repo.root, send_env_files)?;
    let seed = repo.seed_key();
    let manifest = transport.output(&remote::invocation("manifest", &[remote_root, &seed]))?;
    let remote_entries = parse_manifest(&manifest);
    let plan = diff(&local, &remote_entries);
    let to_send: Vec<&LocalFile> = plan.send.iter().map(|&i| &local[i]).collect();
    let stats = Stats {
        files: local.len(),
        sent: to_send.len(),
        bytes: to_send.iter().map(|f| f.size).sum(),
        deleted: plan.delete.len(),
    };
    tracing::info!(?stats, remote_files = remote_entries.len(), "sync plan");
    let label = Label {
        kind: "seed",
        repo: &repo.name,
        repo_id: &repo.id,
        worktree: repo.root.to_string_lossy().into_owned(),
        client: &repo.client,
        updated: crate::state::now_secs(),
    };
    let b64 = base64::engine::general_purpose::STANDARD;
    let meta = b64.encode(serde_json::to_vec(&label).unwrap_or_default());
    let mut deletes = Vec::new();
    for d in &plan.delete {
        deletes.extend_from_slice(d.as_bytes());
        deletes.push(0);
    }
    let deletes = b64.encode(deletes);
    let cmd = remote::invocation("receive", &[remote_root, &seed, &meta, &deletes]);
    transport.feed(&cmd, &mut |w| {
        write_tar(&repo.root, &to_send, w).map(|_| ())
    })?;
    tracing::info!(?stats, elapsed = ?started.elapsed(), "synced");
    Ok(stats)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo::git;

    fn init(dir: &Path) {
        for args in [
            vec!["init", "-q", "-b", "main"],
            vec!["config", "user.email", "t@example.com"],
            vec!["config", "user.name", "t"],
        ] {
            git(dir, &args).unwrap();
        }
    }

    #[test]
    fn file_set_is_the_user_visible_tree() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        init(root);
        for (p, c) in [
            ("tracked.rs", "t"),
            ("modified.rs", "m"),
            ("gone.rs", "g"),
            (".gitignore", "target/\n*.log\n"),
            ("sub/.env", "SECRET=placeholder"),
            (".env.local", "SECRET=placeholder"),
        ] {
            let path = root.join(p);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, c).unwrap();
        }
        git(root, &["add", "-f", "."]).unwrap();
        git(root, &["commit", "-qm", "init"]).unwrap();
        std::fs::write(root.join("modified.rs"), "changed").unwrap();
        std::fs::remove_file(root.join("gone.rs")).unwrap();
        std::fs::write(root.join("untracked.rs"), "u").unwrap();
        std::fs::create_dir_all(root.join("target")).unwrap();
        std::fs::write(root.join("target/big"), "x").unwrap();
        std::fs::write(root.join("x.log"), "x").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("tracked.rs", root.join("link.rs")).unwrap();

        let paths: Vec<String> = file_set(root, false)
            .unwrap()
            .into_iter()
            .map(|f| f.path)
            .collect();
        let mut want = vec![".gitignore", "modified.rs", "tracked.rs", "untracked.rs"];
        if cfg!(unix) {
            want.push("link.rs");
        }
        want.sort_unstable();
        assert_eq!(paths, want);

        let with_env: Vec<String> = file_set(root, true)
            .unwrap()
            .into_iter()
            .map(|f| f.path)
            .collect();
        assert!(with_env.contains(&"sub/.env".to_owned()));
        assert!(with_env.contains(&".env.local".to_owned()));
    }

    #[test]
    fn env_file_detection() {
        assert!(is_env_file(".env"));
        assert!(is_env_file("a/b/.env.production"));
        assert!(!is_env_file("src/env.rs"));
        assert!(!is_env_file(".envrc"));
    }

    fn file(path: &str, size: u64, mtime: u64) -> LocalFile {
        LocalFile {
            path: path.to_owned(),
            size,
            mtime,
            kind: Kind::File { exec: false },
        }
    }

    #[test]
    fn diff_sends_changed_and_deletes_gone() {
        let local = vec![
            file("same", 1, 10),
            file("newer", 1, 11),
            file("bigger", 2, 10),
            file("new", 1, 10),
            LocalFile {
                kind: Kind::File { exec: true },
                ..file("chmod", 1, 10)
            },
        ];
        let manifest = b"f\t1\t10.123\t644\t\tsame\0f\t1\t10.0\t644\t\tnewer\0f\t1\t10.0\t644\t\tbigger\0f\t1\t10.0\t644\t\tchmod\0f\t1\t1.0\t644\t\tgone\0l\t3\t1.0\t777\tsame\tgone link\0";
        let remote = parse_manifest(manifest);
        assert_eq!(remote.len(), 6);
        let plan = diff(&local, &remote);
        let sent: Vec<&str> = plan.send.iter().map(|&i| local[i].path.as_str()).collect();
        assert_eq!(sent, ["newer", "bigger", "new", "chmod"]);
        assert_eq!(plan.delete, ["gone", "gone link"]);
    }

    // frob:tests crates/goway/src/sync.rs::write_tar
    #[test]
    fn tar_round_trips_mtime_and_exec_bit() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a"), "hello").unwrap();
        let f = LocalFile {
            kind: Kind::File { exec: true },
            ..file("a", 5, 1_700_000_000)
        };
        let l = LocalFile {
            path: "l".to_owned(),
            size: 0,
            mtime: 1_700_000_001,
            kind: Kind::Symlink {
                target: "a".to_owned(),
            },
        };
        let bytes = write_tar(dir.path(), &[&f, &l], Vec::new()).unwrap();
        let mut archive = tar::Archive::new(bytes.as_slice());
        let entries: Vec<(String, u64, u32)> = archive
            .entries()
            .unwrap()
            .map(|e| {
                let e = e.unwrap();
                let h = e.header();
                (
                    e.path().unwrap().to_string_lossy().into_owned(),
                    h.mtime().unwrap(),
                    h.mode().unwrap(),
                )
            })
            .collect();
        assert_eq!(
            entries,
            [
                ("a".to_owned(), 1_700_000_000, 0o755),
                ("l".to_owned(), 1_700_000_001, 0o777)
            ]
        );
    }

    /// Runs the remote script through a local shell.
    struct LocalTransport;

    impl Transport for LocalTransport {
        fn output(&self, cmd: &str) -> Result<Vec<u8>> {
            let out = std::process::Command::new("sh")
                .args(["-c", cmd])
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
            Ok(out.stdout)
        }
        fn feed(
            &self,
            cmd: &str,
            feed: &mut dyn FnMut(&mut dyn std::io::Write) -> Result<()>,
        ) -> Result<()> {
            let mut c = std::process::Command::new("sh");
            c.args(["-c", cmd]);
            feed_child(c, feed)
        }
    }

    fn tree(dir: &Path) -> BTreeMap<String, (String, u64)> {
        let mut out = BTreeMap::new();
        let mut stack = vec![dir.to_path_buf()];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).unwrap() {
                let p = e.unwrap().path();
                let meta = std::fs::symlink_metadata(&p).unwrap();
                if meta.is_dir() {
                    stack.push(p);
                    continue;
                }
                let rel = p.strip_prefix(dir).unwrap().to_string_lossy().into_owned();
                let content = if meta.file_type().is_symlink() {
                    format!("-> {}", std::fs::read_link(&p).unwrap().display())
                } else {
                    std::fs::read_to_string(&p).unwrap()
                };
                let mtime = meta
                    .modified()
                    .unwrap()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs();
                out.insert(rel, (content, mtime));
            }
        }
        out
    }

    // frob:tests crates/goway/src/sync.rs::sync
    // frob:tests crates/goway/src/sync.rs::feed_child
    #[cfg(unix)]
    #[test]
    fn protocol_converges_and_keeps_snapshots_isolated() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("proj");
        std::fs::create_dir(&root).unwrap();
        init(&root);
        std::fs::create_dir_all(root.join("src/deep")).unwrap();
        std::fs::write(root.join("src/lib.rs"), "v1").unwrap();
        std::fs::write(root.join("src/deep/x.rs"), "x").unwrap();
        std::fs::write(root.join("keep.rs"), "k").unwrap();
        std::fs::write(root.join(".env"), "SECRET=placeholder").unwrap();
        std::os::unix::fs::symlink("keep.rs", root.join("link")).unwrap();
        let repo = Repo::discover(&root).unwrap();
        let remote_root = dir.path().join("remote").to_string_lossy().into_owned();
        let seed_tree = dir
            .path()
            .join("remote/seed")
            .join(repo.seed_key())
            .join("tree");

        let first = sync(&LocalTransport, &remote_root, &repo, false).unwrap();
        assert_eq!((first.files, first.sent, first.deleted), (4, 4, 0));
        let mut local = tree(&root);
        local.remove(".env");
        local.retain(|k, _| !k.starts_with(".git/") && k != ".git");
        assert_eq!(tree(&seed_tree), local, "content and mtimes match");

        let again = sync(&LocalTransport, &remote_root, &repo, false).unwrap();
        assert_eq!(
            (again.sent, again.deleted),
            (0, 0),
            "second sync is a no-op"
        );

        // A run's snapshot is a hard-link copy; a later sync must not change it.
        let snap = dir.path().join("snap");
        let ok = std::process::Command::new("cp")
            .arg("-al")
            .arg(&seed_tree)
            .arg(&snap)
            .status()
            .unwrap();
        assert!(ok.success());
        std::fs::write(root.join("src/lib.rs"), "v2 longer").unwrap();
        std::fs::remove_dir_all(root.join("src/deep")).unwrap();
        let third = sync(&LocalTransport, &remote_root, &repo, false).unwrap();
        assert_eq!((third.sent, third.deleted), (1, 1));
        assert_eq!(
            std::fs::read_to_string(seed_tree.join("src/lib.rs")).unwrap(),
            "v2 longer"
        );
        assert!(!seed_tree.join("src/deep").exists(), "emptied dirs pruned");
        assert_eq!(
            std::fs::read_to_string(snap.join("src/lib.rs")).unwrap(),
            "v1"
        );
        let label = std::fs::read_to_string(seed_tree.parent().unwrap().join("meta.json")).unwrap();
        assert!(label.contains("\"kind\":\"seed\""), "{label}");
    }

    /// Live check against a real host: `GOWAY_LIVE_HOST=name` (configured
    /// with `goway host add`); skipped otherwise.
    #[test]
    fn live_sync_against_a_real_host() {
        let Ok(name) = std::env::var("GOWAY_LIVE_HOST") else {
            return;
        };
        let paths = crate::paths::Paths::from_env();
        let config = crate::config::Config::load(&paths.config_file()).unwrap();
        let host = config.host(&name).unwrap().clone();
        let mut state = crate::state::State::load(&paths.state_file()).unwrap();
        let settings = ssh::Settings::from_paths(&paths);
        let found = crate::resolve::resolve(
            &config,
            &host,
            &mut state,
            &crate::resolve::SystemLookup,
            &crate::resolve::SshProber {
                settings: settings.clone(),
            },
            KeyPolicy::Strict,
            "true",
        )
        .unwrap();
        let dir = tempfile::tempdir().unwrap();
        init(dir.path());
        std::fs::write(dir.path().join("a.txt"), "live").unwrap();
        let repo = Repo::discover(dir.path()).unwrap();
        let transport = SshTransport {
            target: &found.target,
            settings: &settings,
        };
        let root = ".cache/goway-test";
        let first = sync(&transport, root, &repo, false).unwrap();
        assert_eq!(first.sent, 1);
        let again = sync(&transport, root, &repo, false).unwrap();
        assert_eq!(again.sent, 0);
        transport
            .output(&format!("rm -rf {root}/seed/{}", repo.id))
            .unwrap();
    }
}
