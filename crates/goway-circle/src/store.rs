//! The on-disk circle: `<base>/<id>/ops.jsonl` and `heads.json`, with anti-rollback.

use std::collections::BTreeSet;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use crate::error::StoreError;
use crate::op::{Kind, MAX_OP_BYTES, MAX_OPS, Op, parse_ops};
use crate::state::{accept_roster, heads};

/// Where file writes go; a host program journals them, tests write plainly.
pub trait FileSink {
    /// Write `bytes` to `path` atomically, creating parent directories.
    fn write(&self, path: &Path, bytes: &[u8]) -> std::io::Result<()>;
}

/// A [`FileSink`] that only writes (temp file then rename).
#[derive(Debug, Default, Clone, Copy)]
pub struct PlainSink;

impl FileSink for PlainSink {
    fn write(&self, path: &Path, bytes: &[u8]) -> std::io::Result<()> {
        let dir = path.parent().unwrap_or_else(|| Path::new("."));
        std::fs::create_dir_all(dir)?;
        let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
        tmp.write_all(bytes)?;
        tmp.persist(path).map_err(|e| e.error)?;
        Ok(())
    }
}

/// How long a writer waits for another writer's lock before giving up.
const LOCK_WAIT: std::time::Duration = std::time::Duration::from_secs(60);

/// An exclusive advisory lock on one circle's directory, released on drop.
#[derive(Debug)]
pub struct StoreLock {
    _file: std::fs::File,
}

/// The stored heads, as written to `heads.json`.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct HeadsFile {
    heads: BTreeSet<String>,
}

/// One circle's directory.
#[derive(Debug, Clone)]
pub struct Store {
    dir: PathBuf,
    id: String,
}

/// The directory name of a circle: the first 16 hex digits of its genesis id.
fn short_id(genesis: &str) -> String {
    genesis.chars().take(16).collect()
}

impl Store {
    /// The store for the circle whose genesis op id is `genesis`, under `base`.
    pub fn for_genesis(base: &Path, genesis: &str) -> Self {
        let id = short_id(genesis);
        Self {
            dir: base.join(&id),
            id,
        }
    }

    /// Find the one stored circle under `base`, or the one named `wanted` (an id prefix).
    pub fn find(base: &Path, wanted: Option<&str>) -> Result<Self, StoreError> {
        let entries = match std::fs::read_dir(base) {
            Ok(rd) => rd,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(StoreError::NoCircle),
            Err(e) => {
                return Err(StoreError::Io {
                    op: "list",
                    path: base.to_owned(),
                    source: e,
                });
            }
        };
        let mut ids: Vec<String> = entries
            .filter_map(Result::ok)
            .filter(|e| e.path().join("ops.jsonl").is_file())
            .filter_map(|e| e.file_name().into_string().ok())
            .collect();
        ids.sort();
        if let Some(w) = wanted {
            ids.retain(|i| i.starts_with(w));
        }
        match ids.as_slice() {
            [] => Err(StoreError::NoCircle),
            [one] => Ok(Self {
                dir: base.join(one),
                id: one.clone(),
            }),
            many => Err(StoreError::Ambiguous(many.join(", "))),
        }
    }

    /// The short circle id (also the directory name).
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The circle's directory.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn ops_path(&self) -> PathBuf {
        self.dir.join("ops.jsonl")
    }

    fn heads_path(&self) -> PathBuf {
        self.dir.join("heads.json")
    }

    /// Read and strictly parse the stored operations (empty when none are stored yet).
    pub fn ops(&self) -> Result<Vec<Op>, StoreError> {
        let path = self.ops_path();
        let max = (MAX_OPS * MAX_OP_BYTES) as u64;
        if let Ok(meta) = std::fs::metadata(&path)
            && meta.len() > max
        {
            warn!(path = %path.display(), len = meta.len(), "operations file over the cap");
            return Err(StoreError::OpsTooLarge {
                path,
                len: meta.len(),
                max,
            });
        }
        match std::fs::read_to_string(&path) {
            Ok(text) => Ok(parse_ops(&text)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(source) => Err(StoreError::Io {
                op: "read",
                path,
                source,
            }),
        }
    }

    /// The heads recorded the last time a roster was accepted.
    pub fn seen_heads(&self) -> Result<BTreeSet<String>, StoreError> {
        let path = self.heads_path();
        match std::fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str::<HeadsFile>(&text)
                .map(|h| h.heads)
                .map_err(|e| StoreError::Heads {
                    path,
                    reason: e.to_string(),
                }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(BTreeSet::new()),
            Err(source) => Err(StoreError::Io {
                op: "read",
                path,
                source,
            }),
        }
    }

    /// Take the circle's exclusive writer lock, waiting a bounded time for another writer.
    ///
    /// Hold it across load, merge and commit so a concurrent writer cannot lose an operation.
    pub fn lock(&self) -> Result<StoreLock, StoreError> {
        self.lock_for(LOCK_WAIT)
    }

    /// [`Store::lock`] with an explicit wait.
    fn lock_for(&self, wait: std::time::Duration) -> Result<StoreLock, StoreError> {
        let path = self.dir.join(".lock");
        let io = |source| StoreError::Io {
            op: "lock",
            path: path.clone(),
            source,
        };
        std::fs::create_dir_all(&self.dir).map_err(io)?;
        let file = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(&path)
            .map_err(io)?;
        let start = std::time::Instant::now();
        loop {
            match file.try_lock() {
                Ok(()) => {
                    debug!(circle = %self.id, "circle lock taken");
                    return Ok(StoreLock { _file: file });
                }
                Err(std::fs::TryLockError::WouldBlock) if start.elapsed() < wait => {
                    std::thread::sleep(std::time::Duration::from_millis(25));
                }
                Err(std::fs::TryLockError::WouldBlock) => {
                    warn!(circle = %self.id, "timed out waiting for the circle lock");
                    return Err(io(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "another goway process is updating this circle",
                    )));
                }
                Err(std::fs::TryLockError::Error(e)) => return Err(io(e)),
            }
        }
    }

    /// Accept `offered` as the new roster under the writer lock (taken here).
    ///
    /// Callers that loaded the roster first should hold [`Store::lock`] and use
    /// [`Store::commit_locked`] so the load is inside the lock too.
    pub fn commit(&self, offered: &[Op], sink: &dyn FileSink) -> Result<(), StoreError> {
        let guard = self.lock()?;
        self.commit_locked(&guard, offered, sink)
    }

    /// Accept `offered` as the new roster: refuse a rollback, then write ops and heads.
    ///
    /// The offer must hold exactly one genesis whose id matches this store.
    pub fn commit_locked(
        &self,
        _lock: &StoreLock,
        offered: &[Op],
        sink: &dyn FileSink,
    ) -> Result<(), StoreError> {
        let genesis: Vec<String> = offered
            .iter()
            .filter(|o| o.kind == Kind::Genesis)
            .map(Op::id)
            .collect();
        if genesis.len() != 1 || short_id(&genesis[0]) != self.id {
            return Err(StoreError::NoGenesis);
        }
        accept_roster(&self.seen_heads()?, offered)?;
        let mut text = String::new();
        for op in offered {
            text.push_str(&op.to_line());
            text.push('\n');
        }
        let heads_json = serde_json::to_string_pretty(&HeadsFile {
            heads: heads(offered),
        })
        .unwrap_or_default();
        for (path, bytes) in [
            (self.ops_path(), text.into_bytes()),
            (self.heads_path(), format!("{heads_json}\n").into_bytes()),
        ] {
            sink.write(&path, &bytes).map_err(|source| StoreError::Io {
                op: "write",
                path: path.clone(),
                source,
            })?;
            debug!(file = %path.display(), "circle file written");
        }
        info!(circle = %self.id, ops = offered.len(), "roster stored");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::FakeCrypto;

    fn genesis(admin: &FakeCrypto) -> Op {
        Op::signed(
            Kind::Genesis,
            &[("circle", "home"), ("admin", &admin.0), ("name", "a-admin")],
            &[],
            &[admin],
        )
        .unwrap()
    }

    #[test]
    fn commit_round_trips_and_find_sees_it() {
        let dir = tempfile::tempdir().unwrap();
        let admin = FakeCrypto::new("admin");
        let g = genesis(&admin);
        let store = Store::for_genesis(dir.path(), &g.id());
        store.commit(std::slice::from_ref(&g), &PlainSink).unwrap();
        assert_eq!(store.ops().unwrap(), vec![g.clone()]);
        assert_eq!(store.seen_heads().unwrap(), BTreeSet::from([g.id()]));
        let found = Store::find(dir.path(), None).unwrap();
        assert_eq!(found.id(), store.id());
    }

    // frob:tests crates/goway-circle/src/store.rs::Store
    #[test]
    fn concurrent_writers_serialise_and_none_is_lost() {
        let dir = tempfile::tempdir().unwrap();
        let admin = FakeCrypto::new("admin");
        let g = genesis(&admin);
        let store = Store::for_genesis(dir.path(), &g.id());
        store.commit(std::slice::from_ref(&g), &PlainSink).unwrap();
        let threads: Vec<_> = (0..8)
            .map(|i| {
                let store = store.clone();
                let admin = FakeCrypto::new("admin");
                std::thread::spawn(move || {
                    let guard = store.lock().unwrap();
                    let mut ops = store.ops().unwrap();
                    let name = format!("m{i}");
                    let parents: Vec<String> = heads(&ops).into_iter().collect();
                    let op = Op::signed(
                        Kind::Admit,
                        &[
                            ("key", name.as_str()),
                            ("name", name.as_str()),
                            ("roles", "helper"),
                            ("host_fp", ""),
                        ],
                        &parents,
                        &[&admin],
                    )
                    .unwrap();
                    ops.push(op);
                    std::thread::sleep(std::time::Duration::from_millis(10));
                    store.commit_locked(&guard, &ops, &PlainSink).unwrap();
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        assert_eq!(store.ops().unwrap().len(), 9);
    }

    #[test]
    fn a_held_lock_times_out_a_second_writer() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::for_genesis(dir.path(), "abcdef0123456789");
        let _held = store.lock().unwrap();
        let second = store.lock_for(std::time::Duration::from_millis(100));
        assert!(matches!(second, Err(StoreError::Io { op: "lock", .. })));
    }

    // frob:tests crates/goway-circle/src/store.rs::Store
    #[test]
    fn an_oversized_ops_file_is_refused_unread() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::for_genesis(dir.path(), "abcdef0123456789");
        std::fs::create_dir_all(store.ops_path().parent().unwrap()).unwrap();
        let f = std::fs::File::create(store.ops_path()).unwrap();
        f.set_len((MAX_OPS * MAX_OP_BYTES) as u64 + 1).unwrap();
        assert!(matches!(store.ops(), Err(StoreError::OpsTooLarge { .. })));
    }

    #[test]
    fn rollback_is_refused_and_nothing_is_written() {
        let dir = tempfile::tempdir().unwrap();
        let admin = FakeCrypto::new("admin");
        let g = genesis(&admin);
        let admission = Op::signed(
            Kind::Admit,
            &[
                ("key", "n"),
                ("name", "n"),
                ("roles", "helper"),
                ("host_fp", ""),
            ],
            &[g.id()],
            &[&admin],
        )
        .unwrap();
        let store = Store::for_genesis(dir.path(), &g.id());
        store
            .commit(&[g.clone(), admission.clone()], &PlainSink)
            .unwrap();
        let err = store
            .commit(std::slice::from_ref(&g), &PlainSink)
            .unwrap_err();
        assert!(matches!(err, StoreError::Circle(_)), "{err}");
        assert_eq!(store.ops().unwrap().len(), 2);
    }

    #[test]
    fn a_foreign_genesis_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let a = genesis(&FakeCrypto::new("a"));
        let b = genesis(&FakeCrypto::new("b"));
        let store = Store::for_genesis(dir.path(), &a.id());
        assert!(matches!(
            store.commit(&[b], &PlainSink),
            Err(StoreError::NoGenesis)
        ));
    }

    #[test]
    fn find_with_nothing_or_two_circles() {
        let dir = tempfile::tempdir().unwrap();
        assert!(matches!(
            Store::find(dir.path(), None),
            Err(StoreError::NoCircle)
        ));
        for who in ["a", "b"] {
            let g = genesis(&FakeCrypto::new(who));
            Store::for_genesis(dir.path(), &g.id())
                .commit(&[g], &PlainSink)
                .unwrap();
        }
        assert!(matches!(
            Store::find(dir.path(), None),
            Err(StoreError::Ambiguous(_))
        ));
    }

    #[test]
    fn malformed_heads_file_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let g = genesis(&FakeCrypto::new("a"));
        let store = Store::for_genesis(dir.path(), &g.id());
        store.commit(&[g], &PlainSink).unwrap();
        std::fs::write(store.dir().join("heads.json"), "{\"heads\": 3}").unwrap();
        assert!(matches!(store.seen_heads(), Err(StoreError::Heads { .. })));
    }
}
