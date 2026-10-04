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
use crate::remote::{self, Call};
use crate::repo::{self, Repo};
use crate::ssh::{self, KeyPolicy, Target};
use crate::transport::{self, Kind as HostKind};

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

/// Which secret-looking files may be sent: by default none.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Secrets {
    /// Send every secret-looking file (`send_secret_files = true`).
    pub send_all: bool,
    /// Paths or `*` patterns that may be sent anyway (`secret_allow`).
    pub allow: Vec<String>,
}

impl Secrets {
    /// The policy from the config.
    pub fn from_config(d: &crate::config::Defaults) -> Self {
        Self {
            send_all: d.send_secret_files,
            allow: d.secret_allow.clone(),
        }
    }

    /// Whether `path` stays on this machine.
    pub fn keeps_local(&self, path: &str) -> bool {
        !self.send_all && is_secret_file(path) && !self.allow.iter().any(|p| glob_match(p, path))
    }
}

/// Directories whose contents are credentials wherever they appear.
const SECRET_DIRS: &[&str] = &[
    ".ssh",
    ".aws",
    ".gnupg",
    ".azure",
    ".docker",
    ".kube",
    ".terraform",
    ".gcloud",
    ".oci",
    ".password-store",
];
/// Directory pairs (parent, child) whose contents are credentials.
const SECRET_DIR_PAIRS: &[(&str, &str)] = &[
    (".config", "gh"),
    (".config", "gcloud"),
    (".config", "doctl"),
];
/// File names that hold credentials.
const SECRET_NAMES: &[&str] = &[
    ".env",
    ".envrc",
    ".npmrc",
    ".yarnrc",
    ".yarnrc.yml",
    ".pypirc",
    ".netrc",
    "_netrc",
    ".git-credentials",
    ".pgpass",
    ".htpasswd",
    ".dockercfg",
    ".dockerconfigjson",
    ".vault-token",
    ".my.cnf",
    ".boto",
    ".s3cfg",
    ".terraformrc",
    "rclone.conf",
    ".rclone.conf",
    "master.key",
    "auth.json",
    "secrets.json",
    "secrets.yaml",
    "secrets.yml",
    "secrets.toml",
    "secret.json",
    "secret.yaml",
    "secret.yml",
    "terraform.rc",
];
/// File-name prefixes of credential files (`kubeconfig-prod`, `.env.local`,
/// `id_ed25519`).
const SECRET_PREFIXES: &[&str] = &["kubeconfig", ".env.", "id_"];
/// Extensions and suffixes of key stores, state files and secret env files.
const SECRET_EXTENSIONS: &[&str] = &[
    ".pem",
    ".key",
    ".p12",
    ".pfx",
    ".pkcs12",
    ".p8",
    ".jks",
    ".jceks",
    ".keystore",
    ".ppk",
    ".kdbx",
    ".kdb",
    ".gpg",
    ".pgp",
    ".env",
    ".tfstate",
    ".tfstate.backup",
    ".tfvars",
    ".tfvars.json",
    ".kubeconfig",
];
/// A file whose name mentions "secret" or "credential" is a secret only in
/// data formats, so source files such as `secret_store.rs` are still sent.
const SECRET_WORD_EXTENSIONS: &[&str] = &[
    "",
    "json",
    "yaml",
    "yml",
    "toml",
    "ini",
    "conf",
    "cfg",
    "txt",
    "xml",
    "properties",
    "enc",
];

/// Whether `path` looks like a secret goway does not send by default
/// (case-insensitive): env files, credential files, private keys, key
/// stores, Terraform state and variables, kubeconfigs, and anything under
/// `.ssh`, `.aws`, `.config/gh` or similar.
pub fn is_secret_file(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    let mut parts: Vec<&str> = lower.split('/').collect();
    let base = parts.pop().unwrap_or_default();
    let public_key = Path::new(base)
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("pub"));
    let ext = Path::new(base)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default();
    parts.iter().any(|d| SECRET_DIRS.contains(d))
        || parts
            .windows(2)
            .any(|w| SECRET_DIR_PAIRS.contains(&(w[0], w[1])))
        || SECRET_NAMES.contains(&base)
        || (!public_key && SECRET_PREFIXES.iter().any(|p| base.starts_with(p)))
        || SECRET_EXTENSIONS.iter().any(|e| base.ends_with(e))
        || (ext == "json"
            && (base.starts_with("service-account") || base.starts_with("service_account")))
        || ((base.contains("secret") || base.contains("credential"))
            && SECRET_WORD_EXTENSIONS.contains(&ext))
}

/// Match `path` against `pattern`, where `*` stands for any characters.
pub fn glob_match(pattern: &str, path: &str) -> bool {
    let mut parts = pattern.split('*');
    let first = parts.next().unwrap_or_default();
    let Some(mut rest) = path.strip_prefix(first) else {
        return false;
    };
    let pieces: Vec<&str> = parts.collect();
    for (i, piece) in pieces.iter().enumerate() {
        if i + 1 == pieces.len() {
            return rest.ends_with(piece);
        }
        match rest.find(piece) {
            Some(at) => rest = &rest[at + piece.len()..],
            None => return false,
        }
    }
    rest.is_empty()
}

/// The work tree's file set, and what was deliberately left out.
#[derive(Debug, Clone, Default)]
pub struct FileSet {
    /// Files to sync, sorted by path.
    pub files: Vec<LocalFile>,
    /// Secret-looking files kept on this machine.
    pub kept_local: Vec<String>,
    /// Files under a symlinked directory, never read.
    pub behind_links: Vec<String>,
}

/// Whether a parent directory of `path` (relative to `root`) is a symlink;
/// such files are never read (they may point outside the work tree).
fn under_symlink(root: &Path, path: &str, checked: &mut BTreeMap<String, bool>) -> bool {
    let mut dir = String::new();
    let components: Vec<&str> = path.split('/').collect();
    for part in &components[..components.len().saturating_sub(1)] {
        if !dir.is_empty() {
            dir.push('/');
        }
        dir.push_str(part);
        let linked = *checked.entry(dir.clone()).or_insert_with(|| {
            std::fs::symlink_metadata(root.join(&dir)).is_ok_and(|m| is_link_like(&m))
        });
        if linked {
            return true;
        }
    }
    false
}

/// Whether `meta` is a link or an NTFS reparse point (junction, mount
/// point): neither is ever followed out of the work tree.
fn is_link_like(meta: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt as _;
        const REPARSE_POINT: u32 = 0x400;
        if meta.file_attributes() & REPARSE_POINT != 0 {
            return true;
        }
    }
    meta.file_type().is_symlink()
}

/// Open `full` for archiving and prove the handle is the regular file
/// the listing saw: not swapped for a link since, and not reached through
/// a link. (The handle's own metadata is compared with the path's, so a
/// swap between the listing and the open cannot read outside the tree.)
fn open_regular(root: &Path, rel: &str, full: &Path) -> std::io::Result<(std::fs::File, u64)> {
    let file = std::fs::File::open(full)?;
    let meta = file.metadata()?;
    let by_path = std::fs::symlink_metadata(full)?;
    let mut checked = BTreeMap::new();
    let swapped = || std::io::Error::other("changed while syncing: not the regular file listed");
    if !meta.is_file() || is_link_like(&by_path) || under_symlink(root, rel, &mut checked) {
        return Err(swapped());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        if (meta.dev(), meta.ino()) != (by_path.dev(), by_path.ino()) {
            return Err(swapped());
        }
    }
    Ok((file, meta.len()))
}

/// The work tree's file set: what git shows, minus deleted files, secrets
/// (per `secrets`) and anything under a symlinked directory.
pub fn file_set(root: &Path, secrets: &Secrets) -> Result<FileSet> {
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
    let mut set = FileSet::default();
    let mut checked = BTreeMap::new();
    for raw in out.stdout.split(|b| *b == 0).filter(|p| !p.is_empty()) {
        let Ok(path) = String::from_utf8(raw.to_vec()) else {
            return Err(Error::Usage(format!(
                "`{}` has a file name that is not valid UTF-8; goway cannot copy it faithfully (rename it or add it to .gitignore)",
                String::from_utf8_lossy(raw)
            )));
        };
        if !seen.insert(path.clone()) {
            continue;
        }
        if secrets.keeps_local(&path) {
            tracing::debug!(path, "secret-looking file kept local");
            set.kept_local.push(path);
            continue;
        }
        if under_symlink(root, &path, &mut checked) {
            tracing::warn!(path, "file under a symlinked directory not sent");
            set.behind_links.push(path);
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
        set.files.push(LocalFile {
            path,
            size,
            mtime,
            kind,
        });
    }
    if !set.kept_local.is_empty() {
        tracing::info!(
            kept = set.kept_local.len(),
            "secret-looking files kept local"
        );
    }
    set.files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(set)
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
        if ty == "G" {
            continue;
        }
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

/// The seed generation named in a manifest ("" when there is no tree).
pub fn manifest_generation(bytes: &[u8]) -> String {
    bytes
        .split(|b| *b == 0)
        .find_map(|rec| {
            let rec = String::from_utf8_lossy(rec);
            let rest = rec.strip_prefix("G\t")?;
            Some(rest.rsplit('\t').next().unwrap_or("").to_owned())
        })
        .unwrap_or_default()
}

/// The run's work dir, snapshotted from the seed in the same critical
/// section as the upload.
#[derive(Debug, Clone)]
pub struct Snapshot {
    /// The run id (work dir name).
    pub run_id: String,
    /// The work dir's `meta.json`, base64.
    pub meta_b64: String,
    /// Keep the work dir after the run.
    pub keep: bool,
}

/// What to send and what to delete.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Plan {
    /// Indices into the local file list of entries to send.
    pub send: Vec<usize>,
    /// Remote paths to delete.
    pub delete: Vec<String>,
    /// Indices of files whose size and mode match but mtime differs: sent
    /// only if their content differs (a new worktree has new mtimes).
    pub verify: Vec<usize>,
}

/// Compare the local file set with the remote manifest.
pub fn diff(local: &[LocalFile], remote: &BTreeMap<String, RemoteEntry>) -> Plan {
    let mut plan = Plan::default();
    let mut local_paths = BTreeSet::new();
    for (i, f) in local.iter().enumerate() {
        local_paths.insert(f.path.as_str());
        match remote.get(&f.path).map(|r| (&f.kind, r)) {
            Some((
                Kind::Symlink { target: a },
                RemoteEntry {
                    kind: Kind::Symlink { target: b },
                    ..
                },
            )) if a == b => {}
            Some((
                Kind::File { exec: a },
                r @ RemoteEntry {
                    kind: Kind::File { exec: b },
                    ..
                },
            )) if a == b && f.size == r.size => {
                if f.mtime != r.mtime {
                    plan.verify.push(i);
                }
            }
            _ => plan.send.push(i),
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
///
/// Mtimes are whole seconds, so two same-size edits within one second look
/// identical. A file whose mtime is at or after `racy_from` (it may still
/// change within that second) is stored one second older than it is: the
/// next sync then sees a different mtime and compares content, so a later
/// same-second edit is never missed (the same rule git applies to its index).
pub fn write_tar<W: std::io::Write>(
    root: &Path,
    files: &[&LocalFile],
    racy_from: u64,
    out: W,
) -> Result<W> {
    let mut builder = tar::Builder::new(out);
    builder.mode(tar::HeaderMode::Deterministic);
    for f in files {
        let mut header = tar::Header::new_gnu();
        header.set_mtime(if f.mtime >= racy_from {
            f.mtime.saturating_sub(1)
        } else {
            f.mtime
        });
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
                let (file, len) =
                    open_regular(root, &f.path, &full).map_err(|e| Error::io("open", &full, e))?;
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
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Stats {
    /// Files in the local set.
    pub files: usize,
    /// Files sent.
    pub sent: usize,
    /// Bytes of file content sent.
    pub bytes: u64,
    /// Remote files deleted.
    pub deleted: usize,
    /// Secret-looking files kept on this machine (not sent).
    pub kept_local: Vec<String>,
    /// Files under a symlinked directory (not sent).
    pub behind_links: Vec<String>,
    /// The files this sync was made from (for the copy-integrity check).
    pub manifest: Manifest,
}

/// The local file set a sync was made from, as it was then.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Manifest(pub Vec<LocalFile>);

impl std::fmt::Debug for Manifest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Manifest({} files)", self.0.len())
    }
}

/// Runs remote script calls; ssh or interop in production, a local shell in tests.
pub trait Transport {
    /// Run `call` and return its stdout.
    fn output(&self, call: &Call) -> Result<Vec<u8>>;
    /// Run `call` with `input` on its stdin and return its stdout.
    fn exchange(&self, call: &Call, input: &[u8]) -> Result<Vec<u8>>;
    /// Run `call`, streaming what `feed` writes into its stdin.
    fn feed(
        &self,
        call: &Call,
        feed: &mut dyn FnMut(&mut dyn std::io::Write) -> Result<()>,
    ) -> Result<()>;
}

/// The production transport: ssh (or WSL interop for the Windows side of
/// this machine) to a resolved target, speaking the host's own language.
#[derive(Debug, Clone)]
pub struct SshTransport<'a> {
    /// How the host is reached and what it speaks.
    pub kind: HostKind,
    /// Where to run.
    pub target: &'a Target,
    /// ssh settings.
    pub settings: &'a ssh::Settings,
}

impl<'a> SshTransport<'a> {
    /// The transport to the host `found` describes.
    pub fn of(found: &'a crate::resolve::Found, settings: &'a ssh::Settings) -> Self {
        Self {
            kind: found.kind,
            target: &found.target,
            settings,
        }
    }

    fn fail(&self, message: String) -> Error {
        Error::Ssh {
            host: self.target.name.clone(),
            message,
        }
    }

    /// The process that runs `call` on this host.
    pub fn command(&self, call: &Call) -> Result<std::process::Command> {
        transport::call_command(
            self.kind,
            self.target,
            self.settings,
            KeyPolicy::Strict,
            call,
        )
    }

    /// Run `attempt`; when a Windows host does not have this version of the
    /// remote script yet, install it and run `attempt` once more.
    fn with_script<T>(&self, mut attempt: impl FnMut() -> Result<T>) -> Result<T> {
        match attempt() {
            Err(Error::Ssh { message, .. })
                if self.kind != HostKind::Unix && message.contains(remote::NOT_INSTALLED_MARK) =>
            {
                tracing::info!(host = %self.target.name, "installing the remote script");
                self.install_script()?;
                attempt()
            }
            other => other,
        }
    }

    /// Install this version of the PowerShell remote script on the host.
    ///
    /// # Errors
    ///
    /// [`Error::Ssh`] when the host cannot be reached or refuses it.
    pub fn install_script(&self) -> Result<()> {
        let cmd = transport::command(
            self.kind,
            self.target,
            self.settings,
            KeyPolicy::Strict,
            transport::Script::Ps(&remote::ps_install()),
        )?;
        exchange_child(cmd, remote::SCRIPT_PS.as_bytes())
            .map(drop)
            .map_err(|e| match e {
                Error::Ssh { message, .. } => self.fail(message),
                other => other,
            })
    }
}

/// Most stdout bytes goway keeps from one helper call (a manifest of a big
/// tree is the largest legitimate answer).
pub const MAX_HELPER_STDOUT: usize = 64 << 20;
/// Most stderr bytes goway keeps from one helper call.
pub const MAX_HELPER_STDERR: usize = 64 << 10;

/// What a helper call printed, bounded: a hostile or broken helper cannot
/// exhaust this machine's memory.
#[derive(Debug)]
pub struct Captured {
    /// Exit status of the child.
    pub status: std::process::ExitStatus,
    /// Stdout, at most `stdout_cap` bytes.
    pub stdout: Vec<u8>,
    /// Stderr, at most `stderr_cap` bytes.
    pub stderr: Vec<u8>,
    /// Stdout had more than `stdout_cap` bytes (the rest was discarded).
    pub stdout_overflow: bool,
}

/// Read at most `cap` bytes of `reader`, then discard the rest so the child
/// never blocks on a full pipe. Returns the bytes and whether any were cut.
fn read_capped(reader: Option<impl std::io::Read>, cap: usize) -> (Vec<u8>, bool) {
    use std::io::Read as _;
    let Some(mut reader) = reader else {
        return (Vec::new(), false);
    };
    let mut kept = Vec::new();
    let _ = (&mut reader).take(cap as u64).read_to_end(&mut kept);
    let over = std::io::copy(&mut reader, &mut std::io::sink()).is_ok_and(|n| n > 0);
    (kept, over)
}

/// Wait for `child` (stdout and stderr piped), keeping at most the given
/// number of bytes of each. Like `Child::wait_with_output`, but bounded.
pub fn capture(
    mut child: std::process::Child,
    stdout_cap: usize,
    stderr_cap: usize,
) -> std::io::Result<Captured> {
    let out = child.stdout.take();
    let err = child.stderr.take();
    let ((stdout, stdout_overflow), (stderr, _)) = std::thread::scope(|s| {
        let o = s.spawn(|| read_capped(out, stdout_cap));
        let e = s.spawn(|| read_capped(err, stderr_cap));
        (o.join().unwrap_or_default(), e.join().unwrap_or_default())
    });
    let status = child.wait()?;
    if stdout_overflow {
        tracing::warn!(
            cap = stdout_cap,
            "helper stdout exceeded the cap and was cut"
        );
    }
    Ok(Captured {
        status,
        stdout,
        stderr,
        stdout_overflow,
    })
}

/// The failure text for a helper call: its stderr, trimmed (already capped).
fn stderr_text(c: &Captured) -> String {
    String::from_utf8_lossy(&c.stderr).trim().to_owned()
}

impl Transport for SshTransport<'_> {
    fn output(&self, call: &Call) -> Result<Vec<u8>> {
        self.with_script(|| {
            let child = self
                .command(call)?
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .map_err(|e| self.fail(format!("cannot run ssh: {e}")))?;
            let out = capture(child, MAX_HELPER_STDOUT, MAX_HELPER_STDERR)
                .map_err(|e| self.fail(format!("cannot run ssh: {e}")))?;
            if !out.status.success() {
                return Err(self.fail(stderr_text(&out)));
            }
            if out.stdout_overflow {
                return Err(
                    self.fail("the helper's answer is larger than goway accepts".to_owned())
                );
            }
            Ok(out.stdout)
        })
    }

    fn exchange(&self, call: &Call, input: &[u8]) -> Result<Vec<u8>> {
        self.with_script(|| {
            exchange_child(self.command(call)?, input).map_err(|e| match e {
                Error::Ssh { message, .. } => self.fail(message),
                other => other,
            })
        })
    }

    fn feed(
        &self,
        call: &Call,
        feed: &mut dyn FnMut(&mut dyn std::io::Write) -> Result<()>,
    ) -> Result<()> {
        self.with_script(|| {
            feed_child(self.command(call)?, &mut *feed).map_err(|e| match e {
                Error::Ssh { message, .. } => self.fail(message),
                other => other,
            })
        })
    }
}

/// Spawn `cmd`, write `input` to its stdin (from a thread, so a large
/// output cannot deadlock), and return its stdout on success.
pub fn exchange_child(mut cmd: std::process::Command, input: &[u8]) -> Result<Vec<u8>> {
    let spawn_err = |e: std::io::Error| Error::Ssh {
        host: String::new(),
        message: format!("cannot spawn: {e}"),
    };
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(spawn_err)?;
    let mut stdin = child.stdin.take();
    let out = std::thread::scope(|scope| {
        scope.spawn(|| {
            if let Some(mut w) = stdin.take() {
                let _ = w.write_all(input);
            }
        });
        capture(child, MAX_HELPER_STDOUT, MAX_HELPER_STDERR)
    })
    .map_err(spawn_err)?;
    if !out.status.success() {
        return Err(Error::Ssh {
            host: String::new(),
            message: stderr_text(&out),
        });
    }
    if out.stdout_overflow {
        return Err(Error::Ssh {
            host: String::new(),
            message: "the helper's answer is larger than goway accepts".to_owned(),
        });
    }
    Ok(out.stdout)
}

/// sha256 of a local file, hex.
pub fn file_sha256(path: &Path) -> Option<String> {
    use sha2::Digest as _;
    let mut file = std::fs::File::open(path).ok()?;
    let mut hasher = sha2::Sha256::new();
    std::io::copy(&mut file, &mut hasher).ok()?;
    Some(hasher.finalize().iter().fold(String::new(), |mut s, b| {
        use std::fmt::Write as _;
        let _ = write!(s, "{b:02x}");
        s
    }))
}

/// Parse `sha256sum -z` output: `hash  path\0` records.
pub fn parse_hashes(bytes: &[u8]) -> BTreeMap<String, String> {
    bytes
        .split(|b| *b == 0)
        .filter_map(|rec| {
            let rec = String::from_utf8_lossy(rec);
            let (hash, path) = rec.split_once("  ")?;
            Some((path.to_owned(), hash.to_owned()))
        })
        .collect()
}

/// Most claims one verification answer may carry.
pub const MAX_CLAIMS: usize = 400_000;
/// Longest path or link target a claim may name.
const MAX_CLAIM_TEXT: usize = 4096;

/// What a helper says one path of its copy holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Claim {
    /// A regular file with this SHA-256 (64 lowercase hex digits).
    File(String),
    /// A symlink to this target.
    Link(String),
}

/// Parse a helper's verification answer: a `goway-verify1` header record,
/// then `f SOH sha256 SOH path` and `l SOH target SOH path` records, each
/// NUL-terminated. Strict and bounded: any malformed record, an oversized
/// path, a hash that is not 64 lowercase hex digits, a path that is not
/// plain relative, or more than [`MAX_CLAIMS`] records is an error, never
/// a guess.
pub fn parse_claims(bytes: &[u8]) -> std::result::Result<Vec<(String, Claim)>, String> {
    let mut records = bytes.split(|b| *b == 0);
    if records.next() != Some(b"goway-verify1") {
        return Err("the answer does not start with the goway-verify1 header".to_owned());
    }
    let mut claims = Vec::new();
    let mut rest: Vec<&[u8]> = records.collect();
    // The split leaves one empty slice after the final terminator.
    if rest.last().is_some_and(|r| r.is_empty()) {
        rest.pop();
    }
    if rest.len() > MAX_CLAIMS {
        return Err(format!("more than {MAX_CLAIMS} claims"));
    }
    for rec in rest {
        let mut parts = rec.splitn(3, |b| *b == 1);
        let (Some(kind), Some(value), Some(path)) = (parts.next(), parts.next(), parts.next())
        else {
            return Err("a claim is not `kind, value, path`".to_owned());
        };
        let text = |b: &[u8]| {
            if b.len() > MAX_CLAIM_TEXT {
                return Err("a claim names an oversized path".to_owned());
            }
            String::from_utf8(b.to_vec()).map_err(|_| "a claim is not UTF-8".to_owned())
        };
        let path = text(path)?;
        if path.is_empty()
            || path.starts_with('/')
            || path
                .split('/')
                .any(|c| c == ".." || c.is_empty() || c == ".")
        {
            return Err(format!(
                "a claim names a path that is not plain and relative: {path:?}"
            ));
        }
        let claim = match kind {
            b"f" => {
                let hash = text(value)?;
                if hash.len() != 64 || !hash.bytes().all(|c| matches!(c, b'0'..=b'9' | b'a'..=b'f'))
                {
                    return Err("a claim's hash is not 64 lowercase hex digits".to_owned());
                }
                Claim::File(hash)
            }
            b"l" => Claim::Link(text(value)?),
            _ => return Err("a claim has an unknown kind".to_owned()),
        };
        claims.push((path, claim));
    }
    Ok(claims)
}

/// How one claim disagreed with this machine's file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Why {
    /// A regular file whose content differs.
    Content,
    /// A symlink or file of the wrong kind, or a different link target.
    Kind,
    /// The helper holds a path this sync never sent.
    Unexpected,
}

/// One path whose copy on the helper is not what this machine has.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mismatch {
    /// The path, relative to the work tree.
    pub path: String,
    /// What differed.
    pub why: Why,
    /// The size here in bytes, when known (never any content).
    pub size: Option<u64>,
}

/// The outcome of comparing a helper's claims with the local files.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Comparison {
    /// Claims compared.
    pub checked: usize,
    /// Claims not compared because the local file changed since the sync.
    pub skipped: usize,
    /// Paths that disagree.
    pub mismatches: Vec<Mismatch>,
}

/// Compare `claims` with the files of `root` as listed in `files` (the sync's
/// manifest). A local file that changed since the sync (size or mtime) is
/// skipped: the helper has the older, correct copy.
pub fn compare_claims(root: &Path, files: &Manifest, claims: &[(String, Claim)]) -> Comparison {
    let by_path: BTreeMap<&str, &LocalFile> =
        files.0.iter().map(|f| (f.path.as_str(), f)).collect();
    let mut out = Comparison::default();
    for (path, claim) in claims {
        let Some(file) = by_path.get(path.as_str()) else {
            out.mismatches.push(Mismatch {
                path: path.clone(),
                why: Why::Unexpected,
                size: None,
            });
            continue;
        };
        let full = root.join(path);
        let meta = std::fs::symlink_metadata(&full).ok();
        let mtime = meta
            .as_ref()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs());
        let unchanged = meta
            .as_ref()
            .is_some_and(|m| m.len() == file.size || file.size == 0)
            && mtime == Some(file.mtime);
        if !unchanged {
            out.skipped += 1;
            continue;
        }
        out.checked += 1;
        let ok = match (&file.kind, claim) {
            (Kind::File { .. }, Claim::File(hash)) => {
                file_sha256(&full).as_deref() == Some(hash.as_str())
            }
            (Kind::Symlink { target }, Claim::Link(got)) => target == got,
            _ => {
                out.mismatches.push(Mismatch {
                    path: path.clone(),
                    why: Why::Kind,
                    size: Some(file.size),
                });
                continue;
            }
        };
        if !ok {
            out.mismatches.push(Mismatch {
                path: path.clone(),
                why: if matches!(file.kind, Kind::File { .. }) {
                    Why::Content
                } else {
                    Why::Kind
                },
                size: Some(file.size),
            });
        }
    }
    out
}

fn nul_list<'a>(items: impl Iterator<Item = &'a str>) -> Vec<u8> {
    let mut out = Vec::new();
    for item in items {
        out.extend_from_slice(item.as_bytes());
        out.push(0);
    }
    out
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
    let out = capture(child, 0, MAX_HELPER_STDERR).map_err(|e| Error::Ssh {
        host: String::new(),
        message: format!("wait failed: {e}"),
    })?;
    if !out.status.success() {
        return Err(Error::Ssh {
            host: String::new(),
            message: stderr_text(&out),
        });
    }
    if let Some(Err(e)) = written {
        return Err(e);
    }
    Ok(())
}

/// How often a sync restarts when the seed changed underneath it.
const SYNC_ATTEMPTS: u32 = 3;

/// Bring the remote seed of `repo` up to date with the local work tree and,
/// with `snapshot`, create the run's work dir from it atomically. Restarts
/// from the manifest when the seed was replaced mid-sync (gc raced it).
pub fn sync(
    transport: &dyn Transport,
    remote_root: &str,
    repo: &Repo,
    secrets: &Secrets,
    snapshot: Option<&Snapshot>,
) -> Result<Stats> {
    let mut attempt = 1;
    loop {
        match sync_once(transport, remote_root, repo, secrets, snapshot) {
            Err(Error::Ssh { message, .. })
                if message.contains("seed changed") && attempt < SYNC_ATTEMPTS =>
            {
                tracing::warn!(attempt, message, "seed changed during sync; starting over");
                attempt += 1;
            }
            other => return other,
        }
    }
}

fn sync_once(
    transport: &dyn Transport,
    remote_root: &str,
    repo: &Repo,
    secrets: &Secrets,
    snapshot: Option<&Snapshot>,
) -> Result<Stats> {
    let started = std::time::Instant::now();
    // Taken before the files are read: a file modified at or after this
    // second may still be modified again within the same second.
    let racy_from = crate::state::now_secs().saturating_sub(1);
    let set = file_set(&repo.root, secrets)?;
    let local = set.files;
    let seed = repo.seed_key();
    let manifest = transport.output(&Call::new("manifest", &[remote_root, &seed]))?;
    let generation = manifest_generation(&manifest);
    let remote_entries = parse_manifest(&manifest);
    let mut plan = diff(&local, &remote_entries);
    let mut unchanged = 0usize;
    if !plan.verify.is_empty() {
        let paths = nul_list(plan.verify.iter().map(|&i| local[i].path.as_str()));
        let remote_hashes =
            parse_hashes(&transport.exchange(&Call::new("hashes", &[remote_root, &seed]), &paths)?);
        for &i in &plan.verify {
            let f = &local[i];
            let same = remote_hashes.get(&f.path).is_some_and(|h| {
                file_sha256(&repo.root.join(&f.path)).as_deref() == Some(h.as_str())
            });
            if same {
                unchanged += 1;
            } else {
                plan.send.push(i);
            }
        }
        tracing::info!(
            checked = plan.verify.len(),
            unchanged,
            "content check of mtime-only changes"
        );
    }
    let to_send: Vec<&LocalFile> = plan.send.iter().map(|&i| &local[i]).collect();
    let stats = Stats {
        files: local.len(),
        sent: to_send.len(),
        bytes: to_send.iter().map(|f| f.size).sum(),
        deleted: plan.delete.len(),
        kept_local: set.kept_local,
        behind_links: set.behind_links,
        manifest: Manifest(local.clone()),
    };
    tracing::info!(?stats, remote_files = remote_entries.len(), "sync plan");
    // Deletions are bound to this attempt: a list stored by an attempt
    // whose upload failed is discarded, never applied later.
    let attempt = format!(
        "{}-{}",
        crate::state::now_secs(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.subsec_nanos())
    );
    if !plan.delete.is_empty() {
        transport.exchange(
            &Call::new("deletions", &[remote_root, &seed, &attempt]),
            &nul_list(plan.delete.iter().map(String::as_str)),
        )?;
    }
    if !to_send.is_empty() {
        transport.exchange(
            &Call::new("changes", &[remote_root, &seed, &attempt]),
            &nul_list(to_send.iter().map(|f| f.path.as_str())),
        )?;
    }
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
    let (run_id, work_meta, keep) = snapshot.map_or(("", "", "0"), |s| {
        (
            s.run_id.as_str(),
            s.meta_b64.as_str(),
            if s.keep { "1" } else { "0" },
        )
    });
    let cmd = Call::new(
        "receive",
        &[
            remote_root,
            &seed,
            &meta,
            &generation,
            run_id,
            work_meta,
            keep,
            &attempt,
        ],
    );
    transport.feed(&cmd, &mut |w| {
        write_tar(&repo.root, &to_send, racy_from, w).map(|_| ())
    })?;
    tracing::info!(?stats, elapsed = ?started.elapsed(), "synced");
    Ok(stats)
}

/// Test support: drive `remote.ps1` under whatever PowerShell the machine has.
#[cfg(test)]
pub(crate) mod pwsh_support {
    use super::*;

    /// The PowerShell the Windows-host tests drive: `GOWAY_PWSH`, else `pwsh`
    /// (Windows CI has `powershell`); without one those tests pass trivially.
    pub(crate) fn pwsh() -> Option<PathBuf> {
        let mut candidates: Vec<PathBuf> = std::env::var_os("GOWAY_PWSH")
            .map(PathBuf::from)
            .into_iter()
            .collect();
        candidates.push(PathBuf::from("pwsh"));
        if cfg!(windows) {
            candidates.push(PathBuf::from("powershell"));
        }
        candidates.into_iter().find(|c| {
            std::process::Command::new(c)
                .args(["-NoProfile", "-Command", "exit 0"])
                .output()
                .is_ok_and(|o| o.status.success())
        })
    }

    /// Runs `remote.ps1` (the Windows side) under PowerShell, calling the
    /// script file directly the way an installed copy would be called.
    pub(crate) struct PwshTransport {
        pub(crate) ps: PathBuf,
        pub(crate) script: PathBuf,
    }

    impl PwshTransport {
        fn command(&self, call: &Call) -> std::process::Command {
            let mut words = vec![
                self.script.to_string_lossy().into_owned(),
                call.verb.clone(),
            ];
            words.extend(call.args.iter().cloned());
            let source = format!("{}; exit $LASTEXITCODE", transport::ps_call(&words));
            let mut c = std::process::Command::new(&self.ps);
            c.args(transport::POWERSHELL_FLAGS)
                .arg(transport::encoded_command(&source));
            c
        }
    }

    impl Transport for PwshTransport {
        fn output(&self, call: &Call) -> Result<Vec<u8>> {
            exchange_child(self.command(call), b"")
        }
        fn exchange(&self, call: &Call, input: &[u8]) -> Result<Vec<u8>> {
            exchange_child(self.command(call), input)
        }
        fn feed(
            &self,
            call: &Call,
            feed: &mut dyn FnMut(&mut dyn std::io::Write) -> Result<()>,
        ) -> Result<()> {
            feed_child(self.command(call), feed)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::pwsh_support::{PwshTransport, pwsh};
    use super::*;
    use crate::repo::git;

    fn init(dir: &Path) {
        for args in [
            vec!["init", "-q", "-b", "main"],
            vec!["config", "user.email", "t@example.com"],
            vec!["config", "user.name", "t"],
            vec!["config", "core.autocrlf", "false"],
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

        let paths: Vec<String> = file_set(root, &Secrets::default())
            .unwrap()
            .files
            .into_iter()
            .map(|f| f.path)
            .collect();
        let mut want = vec![".gitignore", "modified.rs", "tracked.rs", "untracked.rs"];
        if cfg!(unix) {
            want.push("link.rs");
        }
        want.sort_unstable();
        assert_eq!(paths, want);

        let with_env: Vec<String> = file_set(
            root,
            &Secrets {
                send_all: true,
                allow: Vec::new(),
            },
        )
        .unwrap()
        .files
        .into_iter()
        .map(|f| f.path)
        .collect();
        assert!(with_env.contains(&"sub/.env".to_owned()));
        assert!(with_env.contains(&".env.local".to_owned()));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_non_utf8_file_name_fails_loudly() {
        use std::os::unix::ffi::OsStrExt as _;
        let dir = tempfile::tempdir().unwrap();
        init(dir.path());
        let name = std::ffi::OsStr::from_bytes(b"bad\xff.rs");
        std::fs::write(dir.path().join(name), "x").unwrap();
        let e = file_set(dir.path(), &Secrets::default()).unwrap_err();
        assert!(e.to_string().contains("not valid UTF-8"), "{e}");
    }

    #[cfg(unix)]
    #[test]
    fn a_flooding_helper_is_captured_within_the_caps() {
        let child = std::process::Command::new("sh")
            .args([
                "-c",
                "head -c 200000 /dev/zero; head -c 300000 /dev/zero >&2",
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let out = capture(child, 1000, 100).unwrap();
        assert!(out.status.success());
        assert_eq!(out.stdout.len(), 1000);
        assert!(out.stdout_overflow);
        assert_eq!(out.stderr.len(), 100);
        let child = std::process::Command::new("sh")
            .args(["-c", "printf abc"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let out = capture(child, 1000, 100).unwrap();
        assert_eq!(out.stdout, b"abc");
        assert!(!out.stdout_overflow);
    }

    /// Credential files that must never be sent by default (gitleaks and
    /// trufflehog style names plus the common cloud and tool configs).
    const SECRET_CORPUS: &[&str] = &[
        ".env",
        ".ENV",
        "a/b/.env.production",
        "prod.env",
        "config/staging.ENV",
        ".envrc",
        ".npmrc",
        ".yarnrc.yml",
        ".netrc",
        "_netrc",
        ".git-credentials",
        ".pgpass",
        ".htpasswd",
        "id_rsa",
        "keys/id_ed25519",
        "server.PEM",
        "tls/key.key",
        "cert.p12",
        "cert.pfx",
        "apple.p8",
        "store.jks",
        "store.jceks",
        "app.keystore",
        "putty.ppk",
        "vault.kdbx",
        "old.kdb",
        "secrets.tar.gpg",
        "mail.pgp",
        ".aws/credentials",
        "home/.ssh/config",
        "credentials",
        "credentials.json",
        ".cargo/credentials",
        ".cargo/credentials.toml",
        ".gem/credentials",
        "config/credentials.yml.enc",
        "jenkins/credentials.xml",
        "terraform.tfstate",
        "terraform.tfstate.backup",
        "infra/prod.tfvars",
        "infra/a.auto.tfvars.json",
        ".terraform/terraform.tfstate",
        ".terraformrc",
        "kubeconfig",
        "deploy/kubeconfig-prod",
        "admin.kubeconfig",
        ".kube/config",
        ".dockercfg",
        ".docker/config.json",
        ".dockerconfigjson",
        ".vault-token",
        ".my.cnf",
        ".boto",
        ".s3cfg",
        "rclone.conf",
        "config/master.key",
        "secrets.yaml",
        "k8s/secrets.yml",
        "secrets.json",
        "secret.yaml",
        "client_secret_123.json",
        "client_secrets.json",
        "service-account.json",
        "gcp/service_account_key.json",
        "auth.json",
        ".config/gh/hosts.yml",
        ".config/gcloud/credentials.db",
        "app/.gnupg/pubring.kbx",
        "db/secrets.toml",
        "ci/aws_credentials.txt",
    ];

    #[test]
    fn secret_files_are_recognized_case_insensitively() {
        for secret in SECRET_CORPUS {
            assert!(is_secret_file(secret), "{secret}");
        }
        for plain in [
            "src/env.rs",
            "id_rsa.pub",
            "README.md",
            "keyboard.rs",
            "environment.txt",
            "src/credentials.rs",
            "src/secret_store.rs",
            "docs/secrets.md",
            "src/auth.rs",
            "tests/data/keys.txt",
            "gh/readme.md",
            "config/gh.toml",
            ".cargo/config.toml",
            "Cargo.toml",
        ] {
            assert!(!is_secret_file(plain), "{plain}");
        }
        let allow = Secrets {
            send_all: false,
            allow: vec!["tests/fixtures/*.pem".to_owned()],
        };
        assert!(!allow.keeps_local("tests/fixtures/server.pem"));
        assert!(allow.keeps_local("server.pem"));
        assert!(glob_match("*.pem", "a/b.pem") && !glob_match("*.pem", "a.pem.bak"));
    }

    // frob:tests crates/goway/src/sync.rs::file_set
    #[cfg(unix)]
    #[test]
    fn secrets_and_files_under_symlinked_dirs_stay_local() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("proj");
        let outside = dir.path().join("outside");
        std::fs::create_dir_all(root.join("real")).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        init(&root);
        std::fs::write(outside.join("b"), "outside secret").unwrap();
        std::fs::write(root.join("real/b"), "inside").unwrap();
        for f in [".ENV", ".envrc", ".npmrc", "id_rsa", "main.rs"] {
            std::fs::write(root.join(f), "x").unwrap();
        }
        git(&root, &["add", "-f", "."]).unwrap();
        git(&root, &["commit", "-qm", "init"]).unwrap();
        // Replace the tracked directory with a symlink pointing outside.
        std::fs::remove_dir_all(root.join("real")).unwrap();
        std::os::unix::fs::symlink(&outside, root.join("real")).unwrap();
        let set = file_set(&root, &Secrets::default()).unwrap();
        let names: Vec<&str> = set.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(names, ["main.rs", "real"]);
        assert!(
            matches!(set.files[1].kind, Kind::Symlink { .. }),
            "the link itself goes as a link; nothing behind it is read"
        );
        assert_eq!(set.behind_links, ["real/b"]);
        let mut kept = set.kept_local.clone();
        kept.sort();
        assert_eq!(kept, [".ENV", ".envrc", ".npmrc", "id_rsa"]);
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
        assert_eq!(sent, ["bigger", "new", "chmod"]);
        let verify: Vec<&str> = plan
            .verify
            .iter()
            .map(|&i| local[i].path.as_str())
            .collect();
        assert_eq!(
            verify,
            ["newer"],
            "same size, new mtime: content is checked first"
        );
        assert_eq!(plan.delete, ["gone", "gone link"]);
    }

    fn claim(kind: &str, value: &str, path: &str) -> Vec<u8> {
        let mut v = format!("{kind}\u{1}{value}\u{1}{path}").into_bytes();
        v.push(0);
        v
    }

    // frob:tests crates/goway/src/sync.rs::parse_claims
    #[test]
    fn claims_are_parsed_strictly_and_with_bounds() {
        let hash = "a".repeat(64);
        let mut ok = b"goway-verify1\0".to_vec();
        ok.extend(claim("f", &hash, "src/lib.rs"));
        ok.extend(claim("l", "lib.rs", "link"));
        let parsed = parse_claims(&ok).unwrap();
        assert_eq!(
            parsed,
            [
                ("src/lib.rs".to_owned(), Claim::File(hash.clone())),
                ("link".to_owned(), Claim::Link("lib.rs".to_owned())),
            ]
        );
        let with = |rec: Vec<u8>| {
            let mut v = b"goway-verify1\0".to_vec();
            v.extend(rec);
            parse_claims(&v)
        };
        assert!(parse_claims(b"").is_err(), "no header");
        assert!(parse_claims(b"goway-verify2\0").is_err(), "wrong header");
        assert!(
            with(claim("f", &"A".repeat(64), "a")).is_err(),
            "uppercase hex"
        );
        assert!(
            with(claim("f", &"a".repeat(63), "a")).is_err(),
            "short hash"
        );
        assert!(with(claim("x", &hash, "a")).is_err(), "unknown kind");
        assert!(with(claim("f", &hash, "../a")).is_err(), "climbing path");
        assert!(with(claim("f", &hash, "/a")).is_err(), "absolute path");
        assert!(
            with(claim("f", &hash, &"p".repeat(5000))).is_err(),
            "oversized path"
        );
        assert!(with(b"f\x01only-two\0".to_vec()).is_err(), "missing field");
        let mut many = b"goway-verify1\0".to_vec();
        for i in 0..=MAX_CLAIMS {
            many.extend(claim("l", "t", &format!("p{i}")));
        }
        assert!(parse_claims(&many).is_err(), "too many claims");
    }

    // frob:tests crates/goway/src/sync.rs::compare_claims
    #[test]
    fn claims_are_compared_with_local_files_and_stale_ones_skipped() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a"), "hello").unwrap();
        std::fs::write(dir.path().join("b"), "world").unwrap();
        let mtime = |n: &str| {
            std::fs::metadata(dir.path().join(n))
                .unwrap()
                .modified()
                .unwrap()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs()
        };
        let manifest = Manifest(vec![
            LocalFile {
                kind: Kind::File { exec: false },
                ..file("a", 5, mtime("a"))
            },
            LocalFile {
                kind: Kind::File { exec: false },
                ..file("b", 5, mtime("b"))
            },
        ]);
        let good = file_sha256(&dir.path().join("a")).unwrap();
        let claims = vec![
            ("a".to_owned(), Claim::File(good.clone())),
            ("b".to_owned(), Claim::File("0".repeat(64))),
            ("ghost".to_owned(), Claim::File(good.clone())),
        ];
        let c = compare_claims(dir.path(), &manifest, &claims);
        assert_eq!(c.checked, 2);
        let bad: Vec<(&str, &Why)> = c
            .mismatches
            .iter()
            .map(|m| (m.path.as_str(), &m.why))
            .collect();
        assert_eq!(bad, [("b", &Why::Content), ("ghost", &Why::Unexpected)]);
        // A file edited here after the sync is not the helper's fault.
        std::fs::write(dir.path().join("b"), "changed later, longer").unwrap();
        let c = compare_claims(dir.path(), &manifest, &claims[1..2]);
        assert_eq!((c.checked, c.skipped, c.mismatches.len()), (0, 1, 0));
    }

    // frob:tests crates/goway/src/sync.rs::write_tar
    #[cfg(unix)]
    #[test]
    fn a_file_swapped_for_a_link_after_listing_is_never_archived() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret"), "top secret").unwrap();
        std::fs::create_dir(dir.path().join("d")).unwrap();
        std::fs::write(dir.path().join("a"), "x").unwrap();
        std::fs::write(dir.path().join("d/b"), "y").unwrap();
        let a = file("a", 1, 1_700_000_000);
        let b = file("d/b", 1, 1_700_000_000);
        assert!(write_tar(dir.path(), &[&a, &b], u64::MAX, Vec::new()).is_ok());
        // The file becomes a link to a file outside the tree.
        std::fs::remove_file(dir.path().join("a")).unwrap();
        std::os::unix::fs::symlink(outside.path().join("secret"), dir.path().join("a")).unwrap();
        let err = write_tar(dir.path(), &[&a], u64::MAX, Vec::new()).unwrap_err();
        assert!(err.to_string().contains("changed while syncing"), "{err}");
        // The parent directory becomes a link to a directory outside it.
        std::fs::remove_file(dir.path().join("a")).unwrap();
        std::fs::write(outside.path().join("b"), "top secret").unwrap();
        std::fs::remove_dir_all(dir.path().join("d")).unwrap();
        std::os::unix::fs::symlink(outside.path(), dir.path().join("d")).unwrap();
        let err = write_tar(dir.path(), &[&b], u64::MAX, Vec::new()).unwrap_err();
        assert!(err.to_string().contains("changed while syncing"), "{err}");
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
        let bytes = write_tar(dir.path(), &[&f, &l], u64::MAX, Vec::new()).unwrap();
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
    #[cfg(unix)]
    struct LocalTransport;

    #[cfg(unix)]
    impl Transport for LocalTransport {
        fn output(&self, call: &Call) -> Result<Vec<u8>> {
            let out = std::process::Command::new("sh")
                .args(["-c", &call.bash()])
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
            Ok(out.stdout)
        }
        fn exchange(&self, call: &Call, input: &[u8]) -> Result<Vec<u8>> {
            let mut c = std::process::Command::new("sh");
            c.args(["-c", &call.bash()]);
            exchange_child(c, input)
        }
        fn feed(
            &self,
            call: &Call,
            feed: &mut dyn FnMut(&mut dyn std::io::Write) -> Result<()>,
        ) -> Result<()> {
            let mut c = std::process::Command::new("sh");
            c.args(["-c", &call.bash()]);
            feed_child(c, feed)
        }
    }

    #[cfg(unix)]
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
    #[cfg_attr(
        target_os = "macos",
        ignore = "runs the Linux helper script (flock); macOS is not a helper"
    )]
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

        let first = sync(
            &LocalTransport,
            &remote_root,
            &repo,
            &Secrets::default(),
            None,
        )
        .unwrap();
        assert_eq!((first.files, first.sent, first.deleted), (4, 4, 0));
        let mut local = tree(&root);
        local.remove(".env");
        local.retain(|k, _| !k.starts_with(".git/") && k != ".git");
        // Files modified within the last second are stored one second
        // older (see write_tar), so mtimes match to within that second.
        let seeded = tree(&seed_tree);
        assert_eq!(
            seeded.keys().collect::<Vec<_>>(),
            local.keys().collect::<Vec<_>>()
        );
        for (path, (content, mtime)) in &local {
            let (got, got_mtime) = &seeded[path];
            assert_eq!(got, content, "{path}");
            assert!(
                *got_mtime == *mtime || got_mtime + 1 == *mtime,
                "{path}: {got_mtime} vs {mtime}"
            );
        }

        let again = sync(
            &LocalTransport,
            &remote_root,
            &repo,
            &Secrets::default(),
            None,
        )
        .unwrap();
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
        let third = sync(
            &LocalTransport,
            &remote_root,
            &repo,
            &Secrets::default(),
            None,
        )
        .unwrap();
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
        let found_kind = HostKind::of(&host);
        let mut state = crate::state::State::load(&paths.state_file()).unwrap();
        let settings = ssh::Settings::from_paths(&paths);
        let found = crate::resolve::resolve_call(
            &config,
            &host,
            &mut state,
            &crate::resolve::SystemLookup,
            &crate::resolve::SshProber {
                settings: settings.clone(),
            },
            KeyPolicy::Strict,
            &Call::new("ping", &[] as &[&str]),
        )
        .unwrap();
        let dir = tempfile::tempdir().unwrap();
        init(dir.path());
        std::fs::write(dir.path().join("a.txt"), "live").unwrap();
        let repo = Repo::discover(dir.path()).unwrap();
        let transport = SshTransport {
            kind: found_kind,
            target: &found.target,
            settings: &settings,
        };
        let root = ".cache/goway-test";
        let first = sync(&transport, root, &repo, &Secrets::default(), None).unwrap();
        assert_eq!(first.sent, 1);
        let again = sync(&transport, root, &repo, &Secrets::default(), None).unwrap();
        assert_eq!(again.sent, 0);
        transport.output(&Call::new("purge", &[root])).unwrap();
    }

    // frob:tests crates/goway/src/sync.rs::exchange_child
    // frob:tests crates/goway/src/sync.rs::file_sha256
    // frob:tests crates/goway/src/sync.rs::parse_hashes
    #[cfg(unix)]
    #[test]
    #[cfg_attr(
        target_os = "macos",
        ignore = "runs the Linux helper script (flock); macOS is not a helper"
    )]
    fn a_second_worktree_only_sends_what_differs() {
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("proj");
        std::fs::create_dir(&main).unwrap();
        init(&main);
        for i in 0..20 {
            std::fs::write(main.join(format!("f{i}.rs")), format!("fn f{i}() {{}}\n")).unwrap();
        }
        git(&main, &["add", "."]).unwrap();
        git(&main, &["commit", "-qm", "init"]).unwrap();
        let remote_root = dir.path().join("remote").to_string_lossy().into_owned();
        let first = sync(
            &LocalTransport,
            &remote_root,
            &Repo::discover(&main).unwrap(),
            &Secrets::default(),
            None,
        )
        .unwrap();
        assert_eq!(first.sent, 20);

        // A new worktree: same content, new mtimes, one edit, one new file.
        std::thread::sleep(std::time::Duration::from_millis(1100));
        let wt = dir.path().join("wt");
        git(&main, &["worktree", "add", "-q", wt.to_str().unwrap()]).unwrap();
        std::fs::write(wt.join("f3.rs"), "fn changed() {}\n").unwrap();
        std::fs::write(wt.join("new.rs"), "fn new() {}\n").unwrap();
        let repo = Repo::discover(&wt).unwrap();
        let second = sync(
            &LocalTransport,
            &remote_root,
            &repo,
            &Secrets::default(),
            None,
        )
        .unwrap();
        assert_eq!(
            (second.sent, second.deleted),
            (2, 0),
            "only f3.rs and new.rs"
        );
        let seed = dir
            .path()
            .join("remote/seed")
            .join(repo.seed_key())
            .join("tree");
        assert_eq!(
            std::fs::read_to_string(seed.join("f3.rs")).unwrap(),
            "fn changed() {}\n"
        );
        let main_seed = dir
            .path()
            .join("remote/seed")
            .join(Repo::discover(&main).unwrap().seed_key())
            .join("tree");
        assert_eq!(
            std::fs::read_to_string(main_seed.join("f3.rs")).unwrap(),
            "fn f3() {}\n",
            "the sibling seed is untouched"
        );
    }

    #[cfg(unix)]
    #[test]
    #[cfg_attr(
        target_os = "macos",
        ignore = "runs the Linux helper script (flock); macOS is not a helper"
    )]
    fn deleting_more_paths_than_one_argument_holds_works() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("proj");
        std::fs::create_dir(&root).unwrap();
        init(&root);
        let long = "x".repeat(100);
        for i in 0..1500 {
            std::fs::write(root.join(format!("{long}{i:05}.txt")), "x").unwrap();
        }
        let repo = Repo::discover(&root).unwrap();
        let remote_root = dir.path().join("remote").to_string_lossy().into_owned();
        assert_eq!(
            sync(
                &LocalTransport,
                &remote_root,
                &repo,
                &Secrets::default(),
                None
            )
            .unwrap()
            .sent,
            1500
        );
        for i in 0..1500 {
            std::fs::remove_file(root.join(format!("{long}{i:05}.txt"))).unwrap();
        }
        // 1500 * 110 bytes = 165 KB of paths: more than MAX_ARG_STRLEN (128 KiB).
        let gone = sync(
            &LocalTransport,
            &remote_root,
            &repo,
            &Secrets::default(),
            None,
        )
        .unwrap();
        assert_eq!(gone.deleted, 1500);
        let tree = dir
            .path()
            .join("remote/seed")
            .join(repo.seed_key())
            .join("tree");
        assert_eq!(std::fs::read_dir(tree).map_or(0, Iterator::count), 0);
    }

    /// Deletes the seed (as a racing gc would) right before the first upload.
    #[cfg(unix)]
    struct RacingGc {
        seed: PathBuf,
        raced: std::cell::Cell<bool>,
    }

    #[cfg(unix)]
    impl Transport for RacingGc {
        fn output(&self, call: &Call) -> Result<Vec<u8>> {
            LocalTransport.output(call)
        }
        fn exchange(&self, call: &Call, input: &[u8]) -> Result<Vec<u8>> {
            LocalTransport.exchange(call, input)
        }
        fn feed(
            &self,
            call: &Call,
            feed: &mut dyn FnMut(&mut dyn std::io::Write) -> Result<()>,
        ) -> Result<()> {
            if !self.raced.replace(true) {
                std::fs::remove_dir_all(&self.seed).unwrap();
            }
            LocalTransport.feed(call, feed)
        }
    }

    // frob:tests crates/goway/src/sync.rs::manifest_generation
    #[cfg(unix)]
    #[test]
    #[cfg_attr(
        target_os = "macos",
        ignore = "runs the Linux helper script (flock); macOS is not a helper"
    )]
    fn a_seed_replaced_mid_sync_is_detected_and_resynced_fully() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("proj");
        std::fs::create_dir(&root).unwrap();
        init(&root);
        for i in 0..5 {
            std::fs::write(root.join(format!("f{i}")), format!("{i}")).unwrap();
        }
        let repo = Repo::discover(&root).unwrap();
        let remote_root = dir.path().join("remote").to_string_lossy().into_owned();
        assert_eq!(
            sync(
                &LocalTransport,
                &remote_root,
                &repo,
                &Secrets::default(),
                None
            )
            .unwrap()
            .sent,
            5
        );
        std::fs::write(root.join("f0"), "changed").unwrap();
        let seed = dir.path().join("remote/seed").join(repo.seed_key());
        let racing = RacingGc {
            seed: seed.clone(),
            raced: std::cell::Cell::new(false),
        };
        let stats = sync(&racing, &remote_root, &repo, &Secrets::default(), None).unwrap();
        assert_eq!(
            stats.sent, 5,
            "the retry sends everything, not just the delta"
        );
        let mut names: Vec<String> = std::fs::read_dir(seed.join("tree"))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(
            names,
            ["f0", "f1", "f2", "f3", "f4"],
            "complete tree, never a partial one"
        );
        assert_eq!(
            std::fs::read_to_string(seed.join("tree/f0")).unwrap(),
            "changed"
        );
    }

    /// Fails the first upload (after deletions were stored), like a
    /// dropped connection.
    #[cfg(unix)]
    struct DropFirstUpload {
        dropped: std::cell::Cell<bool>,
    }

    #[cfg(unix)]
    impl Transport for DropFirstUpload {
        fn output(&self, call: &Call) -> Result<Vec<u8>> {
            LocalTransport.output(call)
        }
        fn exchange(&self, call: &Call, input: &[u8]) -> Result<Vec<u8>> {
            LocalTransport.exchange(call, input)
        }
        fn feed(
            &self,
            call: &Call,
            feed: &mut dyn FnMut(&mut dyn std::io::Write) -> Result<()>,
        ) -> Result<()> {
            if !self.dropped.replace(true) {
                return Err(Error::Ssh {
                    host: "test".to_owned(),
                    message: "connection dropped".to_owned(),
                });
            }
            LocalTransport.feed(call, feed)
        }
    }

    #[cfg(unix)]
    #[test]
    #[cfg_attr(
        target_os = "macos",
        ignore = "runs the Linux helper script (flock); macOS is not a helper"
    )]
    fn a_stale_deletion_list_never_deletes_restored_files() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("proj");
        std::fs::create_dir(&root).unwrap();
        init(&root);
        std::fs::write(root.join("a"), "a").unwrap();
        std::fs::write(root.join("b"), "b").unwrap();
        let repo = Repo::discover(&root).unwrap();
        let remote_root = dir.path().join("remote").to_string_lossy().into_owned();
        sync(
            &LocalTransport,
            &remote_root,
            &repo,
            &Secrets::default(),
            None,
        )
        .unwrap();

        // b deleted locally; the sync stores the deletion, then the upload drops.
        let saved = std::fs::read(root.join("b")).unwrap();
        let mtime = std::fs::metadata(root.join("b"))
            .unwrap()
            .modified()
            .unwrap();
        std::fs::remove_file(root.join("b")).unwrap();
        let flaky = DropFirstUpload {
            dropped: std::cell::Cell::new(false),
        };
        assert!(sync(&flaky, &remote_root, &repo, &Secrets::default(), None).is_err());

        // b restored exactly; the next sync has nothing to send.
        std::fs::write(root.join("b"), saved).unwrap();
        std::fs::File::options()
            .write(true)
            .open(root.join("b"))
            .unwrap()
            .set_modified(mtime)
            .unwrap();
        sync(
            &LocalTransport,
            &remote_root,
            &repo,
            &Secrets::default(),
            None,
        )
        .unwrap();
        let tree = dir
            .path()
            .join("remote/seed")
            .join(repo.seed_key())
            .join("tree");
        assert!(
            tree.join("b").is_file(),
            "the stale deletion list was applied"
        );
    }

    // frob:tests crates/goway/src/sync.rs::sync
    // frob:tests crates/goway/src/sync.rs::Transport
    #[test]
    fn a_windows_host_syncs_incrementally_and_deletes_only_inside_its_own_directory() {
        let Some(ps) = pwsh() else { return };
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("remote.ps1");
        std::fs::write(&script, remote::SCRIPT_PS).unwrap();
        let transport = PwshTransport { ps, script };
        let remote_root = dir.path().join("goway-root");
        std::fs::create_dir(&remote_root).unwrap();
        let outside = dir.path().join("outside.txt");
        std::fs::write(&outside, "not goway's").unwrap();
        let remote_root = remote_root.to_string_lossy().into_owned();

        let one = dir.path().join("one");
        std::fs::create_dir(&one).unwrap();
        init(&one);
        for i in 0..4 {
            std::fs::write(one.join(format!("f{i}")), format!("{i}")).unwrap();
        }
        let repo = Repo::discover(&one).unwrap();
        let secrets = Secrets::default();
        let first = sync(&transport, &remote_root, &repo, &secrets, None).unwrap();
        assert_eq!((first.files, first.sent), (4, 4));
        // Repeat: nothing changed, nothing is written.
        let again = sync(&transport, &remote_root, &repo, &secrets, None).unwrap();
        assert_eq!((again.sent, again.deleted), (0, 0));
        // One edit and one removal: exactly those travel.
        std::fs::write(one.join("f1"), "edited, longer").unwrap();
        std::fs::remove_file(one.join("f3")).unwrap();
        let delta = sync(&transport, &remote_root, &repo, &secrets, None).unwrap();
        assert_eq!((delta.sent, delta.deleted), (1, 1));
        let seed = std::path::Path::new(&remote_root)
            .join("seed")
            .join(repo.seed_key())
            .join("tree");
        assert_eq!(
            std::fs::read_to_string(seed.join("f1")).unwrap(),
            "edited, longer"
        );
        assert!(!seed.join("f3").exists());
        assert_eq!(std::fs::read_to_string(&outside).unwrap(), "not goway's");

        // A second work tree has a seed of its own; the first is untouched.
        let two = dir.path().join("two");
        std::fs::create_dir(&two).unwrap();
        init(&two);
        std::fs::write(two.join("g0"), "x").unwrap();
        let other = Repo::discover(&two).unwrap();
        let second = sync(&transport, &remote_root, &other, &secrets, None).unwrap();
        assert_eq!(second.sent, 1);
        assert_ne!(repo.seed_key(), other.seed_key());
        assert!(seed.join("f0").exists() && !seed.join("g0").exists());
    }

    // frob:tests crates/goway/src/sync.rs::SshTransport
    #[test]
    fn the_transport_builds_each_hosts_own_kind_of_process() {
        let target = Target {
            name: "winbox".to_owned(),
            address: "192.0.2.7".to_owned(),
            port: 22,
            user: None,
            identity: None,
        };
        let settings = ssh::Settings {
            known_hosts: PathBuf::from("kh"),
            control_dir: None,
            connect_timeout_secs: 5,
        };
        let call = Call::new("ping", &[] as &[&str]);
        let line = |kind| {
            let t = SshTransport {
                kind,
                target: &target,
                settings: &settings,
            };
            t.command(&call)
                .map(|c| {
                    c.get_args()
                        .last()
                        .map(|a| a.to_string_lossy().into_owned())
                        .unwrap_or_default()
                })
                .unwrap_or_default()
        };
        assert!(line(HostKind::Unix).starts_with("bash -c "));
        assert!(line(HostKind::WindowsSsh).starts_with("powershell -NoProfile"));
    }
}
