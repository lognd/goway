//! The local first-come-first-served queue of a wave of runs.
//!
//! Many `goway run` processes on one laptop (an agent launching a wave)
//! must not all pick the same helper in the instant before any of their
//! jobs shows up in its probe. Two kinds of `flock`ed files in goway's
//! state directory coordinate them, so no daemon is needed:
//!
//! - a [`Ticket`] is a run waiting for a host; tickets sort by creation time, so
//!   the order of arrival is the order of service, and a ticket remembers
//!   which hosts its run could use so a later run is held back only from
//!   hosts an earlier one is waiting for;
//! - a [`Claim`] is a run that has chosen a host and has not yet appeared in
//!   that host's probe (it is still syncing, or has only just started); the
//!   chooser counts claims as jobs and as memory already spoken for.
//!
//! A file whose lock nobody holds belongs to a dead process and is removed.
//! The data of a file lives in sidecar files, because on Windows a locked
//! file cannot be read by another process.

use std::collections::BTreeMap;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, SystemTime};

use crate::config::write_atomic;
use crate::error::{Error, Result};
use crate::paths::Paths;

/// How long a claim keeps counting after its run started (the remote job
/// marker appears within moments; this covers the gap before a probe sees it).
pub const CLAIM_GRACE: Duration = Duration::from_secs(10);

/// A file nobody has locked is only called dead after it is this old, so a
/// file that is just being created and locked is never mistaken for one.
const DEAD_AFTER: Duration = Duration::from_secs(2);

static SEQ: AtomicU32 = AtomicU32::new(0);

/// The queue directory of one goway state directory.
#[derive(Debug, Clone)]
pub struct Queue {
    dir: PathBuf,
    grace: Duration,
}

/// A run waiting for a host; removed when dropped.
#[derive(Debug)]
pub struct Ticket {
    _file: File,
    base: PathBuf,
    id: String,
}

/// A run that chose a host and may not be in its probe yet; removed when dropped.
#[derive(Debug)]
pub struct Claim {
    _file: File,
    base: PathBuf,
    host: String,
}

/// What the queue holds right now, as seen by one waiter.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Snapshot {
    /// The hosts each earlier live ticket could use (`None`: not known yet, so any).
    pub earlier: Vec<Option<Vec<String>>>,
    /// Claims not yet visible in a probe, per host (lowercase).
    pub pending: BTreeMap<String, u32>,
}

impl Snapshot {
    /// How many live waiters are ahead of this one.
    pub fn position(&self) -> usize {
        self.earlier.len()
    }

    /// Whether an earlier waiter could use `host` (and so goes first).
    pub fn held_for_earlier(&self, host: &str) -> bool {
        self.earlier.iter().any(|e| {
            e.as_ref()
                .is_none_or(|hosts| hosts.iter().any(|h| h.eq_ignore_ascii_case(host)))
        })
    }
}

fn nanos() -> u128 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos())
}

fn new_id() -> String {
    format!(
        "{:020}-{}-{:06}",
        nanos(),
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    )
}

fn locked_file(path: &Path) -> Result<File> {
    let file = File::create(path).map_err(|e| Error::io("create", path, e))?;
    file.lock().map_err(|e| Error::io("lock", path, e))?;
    Ok(file)
}

/// Whether the process that made `path` still holds its lock; a dead one is removed.
fn live(path: &Path, sidecars: &[&str]) -> bool {
    let Ok(file) = File::open(path) else {
        return false;
    };
    if file.try_lock().is_err() {
        return true;
    }
    let old = std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok())
        .is_some_and(|age| age >= DEAD_AFTER);
    if old {
        tracing::info!(path = %path.display(), "removing a queue file whose owner died");
        let _ = std::fs::remove_file(path);
        for ext in sidecars {
            let _ = std::fs::remove_file(path.with_extension(ext));
        }
    }
    !old
}

impl Queue {
    /// The queue under goway's state directory.
    pub fn new(paths: &Paths) -> Self {
        Self::at(paths.state_dir.join("queue"))
    }

    /// A queue in `dir` (tests use a temporary one).
    pub fn at(dir: PathBuf) -> Self {
        Self {
            dir,
            grace: CLAIM_GRACE,
        }
    }

    /// The same queue with another [`CLAIM_GRACE`] (tests use a short one).
    #[must_use]
    pub fn with_grace(mut self, grace: Duration) -> Self {
        self.grace = grace;
        self
    }

    fn ensure(&self) -> Result<()> {
        std::fs::create_dir_all(&self.dir).map_err(|e| Error::io("create", &self.dir, e))
    }

    /// Run `f` while holding the queue-wide decision lock, so that reading the
    /// queue and claiming a host happen as one step across processes.
    ///
    /// # Errors
    ///
    /// [`Error::Io`] when the lock file cannot be created or locked.
    pub fn decide<T>(&self, f: impl FnOnce() -> T) -> Result<T> {
        self.ensure()?;
        let path = self.dir.join("decide.lock");
        let file = File::create(&path).map_err(|e| Error::io("create", &path, e))?;
        file.lock().map_err(|e| Error::io("lock", &path, e))?;
        let out = f();
        drop(file);
        Ok(out)
    }

    /// Join the line: a ticket that sorts after every earlier one.
    ///
    /// # Errors
    ///
    /// [`Error::Io`] when the ticket file cannot be created or locked.
    pub fn enter(&self) -> Result<Ticket> {
        self.ensure()?;
        let id = new_id();
        let base = self.dir.join(format!("w-{id}"));
        let file = locked_file(&base.with_extension("ticket"))?;
        tracing::debug!(id, "queue ticket taken");
        Ok(Ticket {
            _file: file,
            base,
            id,
        })
    }

    /// Claim `host` for a run that is about to sync and start.
    ///
    /// # Errors
    ///
    /// [`Error::Io`] when the claim file cannot be created or locked.
    pub fn claim(&self, host: &str) -> Result<Claim> {
        self.ensure()?;
        let base = self.dir.join(format!("c-{}-{host}", new_id()));
        let file = locked_file(&base.with_extension("claim"))?;
        tracing::debug!(host, "host claimed");
        Ok(Claim {
            _file: file,
            base,
            host: host.to_owned(),
        })
    }

    /// The live tickets before `mine` (all of them without one) and the claims still pending.
    pub fn snapshot(&self, mine: Option<&Ticket>) -> Snapshot {
        let mut snap = Snapshot::default();
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return snap;
        };
        let mut tickets: Vec<(String, PathBuf)> = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(name) = path.file_stem().and_then(|s| s.to_str()).map(str::to_owned) else {
                continue;
            };
            match path.extension().and_then(|e| e.to_str()) {
                Some("ticket") => {
                    let id = name.trim_start_matches("w-").to_owned();
                    if mine.is_none_or(|m| id < m.id) && live(&path, &["eligible"]) {
                        tickets.push((id, path));
                    }
                }
                // `c-<ns>-<pid>-<seq>-<host>`: the host is the rest after four dashes.
                Some("claim") if live(&path, &["started"]) && still_pending(&path, self.grace) => {
                    if let Some(host) = name.splitn(5, '-').nth(4) {
                        *snap.pending.entry(host.to_ascii_lowercase()).or_insert(0) += 1;
                    }
                }
                _ => {}
            }
        }
        tickets.sort();
        snap.earlier = tickets
            .into_iter()
            .map(|(_, path)| read_eligible(&path))
            .collect();
        snap
    }
}

fn read_eligible(ticket: &Path) -> Option<Vec<String>> {
    let text = std::fs::read_to_string(ticket.with_extension("eligible")).ok()?;
    Some(text.lines().map(str::to_owned).collect())
}

/// A claim counts until its run has started and the grace period after that has passed.
fn still_pending(claim: &Path, grace: Duration) -> bool {
    match std::fs::metadata(claim.with_extension("started")).and_then(|m| m.modified()) {
        Ok(at) => at.elapsed().is_ok_and(|age| age < grace),
        Err(_) => true,
    }
}

impl Ticket {
    /// Record which hosts this run could use, so later runs are held back only from those.
    ///
    /// # Errors
    ///
    /// [`Error::Io`] when the sidecar file cannot be written.
    pub fn set_eligible(&self, hosts: &[String]) -> Result<()> {
        write_atomic(
            &self.base.with_extension("eligible"),
            hosts.join("\n").as_bytes(),
        )
    }
}

impl Drop for Ticket {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(self.base.with_extension("ticket"));
        let _ = std::fs::remove_file(self.base.with_extension("eligible"));
        tracing::debug!(id = self.id, "queue ticket released");
    }
}

impl Claim {
    /// Note that the run has started: its job will show in the host's probe
    /// within [`CLAIM_GRACE`], after which the claim stops counting.
    pub fn started(&self) {
        if let Err(e) = std::fs::write(self.base.with_extension("started"), b"1") {
            tracing::warn!(error = %e, host = self.host, "cannot mark the claim started");
        }
    }

    /// The claimed host.
    pub fn host(&self) -> &str {
        &self.host
    }
}

impl Drop for Claim {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(self.base.with_extension("claim"));
        let _ = std::fs::remove_file(self.base.with_extension("started"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn queue() -> (tempfile::TempDir, Queue) {
        let dir = tempfile::tempdir().unwrap();
        let q = Queue::at(dir.path().join("queue"));
        (dir, q)
    }

    // frob:tests crates/goway/src/queue.rs::Queue
    #[test]
    fn tickets_are_served_in_arrival_order_and_dropped_ones_vanish() {
        let (_d, q) = queue();
        let a = q.enter().unwrap();
        let b = q.enter().unwrap();
        let c = q.enter().unwrap();
        assert_eq!(q.snapshot(Some(&a)).position(), 0);
        assert_eq!(q.snapshot(Some(&b)).position(), 1);
        assert_eq!(q.snapshot(Some(&c)).position(), 2);
        drop(a);
        assert_eq!(q.snapshot(Some(&c)).position(), 1);
    }

    #[test]
    fn an_earlier_ticket_holds_back_only_the_hosts_it_could_use() {
        let (_d, q) = queue();
        let a = q.enter().unwrap();
        let b = q.enter().unwrap();
        // Unknown eligibility holds back every host.
        assert!(q.snapshot(Some(&b)).held_for_earlier("helios"));
        a.set_eligible(&["Helios".to_owned()]).unwrap();
        let snap = q.snapshot(Some(&b));
        assert!(snap.held_for_earlier("helios"));
        assert!(!snap.held_for_earlier("orion"));
    }

    #[test]
    fn claims_count_until_started_and_graced_and_die_with_their_owner() {
        let (_d, q) = queue();
        let c = q.claim("my-host").unwrap();
        let second = q.claim("my-host").unwrap();
        assert_eq!(q.snapshot(None).pending.get("my-host"), Some(&2));
        c.started();
        // Just started: still counted until a probe can see the job.
        assert_eq!(q.snapshot(None).pending.get("my-host"), Some(&2));
        let started = c.base.with_extension("started");
        let old = SystemTime::now() - CLAIM_GRACE - Duration::from_secs(1);
        File::options()
            .write(true)
            .open(&started)
            .unwrap()
            .set_modified(old)
            .unwrap();
        assert_eq!(q.snapshot(None).pending.get("my-host"), Some(&1));
        drop(c);
        drop(second);
        assert!(q.snapshot(None).pending.is_empty());
    }
}
