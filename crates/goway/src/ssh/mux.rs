//! Sharing ssh `ControlMaster` connections without overrunning a helper's
//! `MaxSessions` (sshd's default is 10 sessions per connection).
//!
//! Many goway processes at once (agent waves) used to ride one master per
//! host, so the eleventh session was refused ("Session open refused by
//! peer") and ssh dropped multiplexing with a noisy warning. Now every
//! process claims a *token* for a host: [`SLOTS`] masters per host, each
//! shared by at most [`PER_SLOT`] processes. The token is an advisory file
//! lock held until the process exits (so a crash frees it by itself). A
//! process that finds every token taken connects without multiplexing,
//! quietly. A socket whose master is gone is removed before use, so ssh
//! replaces it instead of disabling multiplexing.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use sha2::{Digest as _, Sha256};

use super::Target;

/// Masters per host (each a separate connection with its own session budget).
pub const SLOTS: usize = 4;
/// Processes sharing one master; below sshd's default `MaxSessions` of 10
/// so each may hold a couple of sessions at once.
pub const PER_SLOT: usize = 4;

/// What a process claimed for one host.
enum Claim {
    /// Use this master slot (the lock file is held for the process lifetime).
    Slot(usize, #[allow(dead_code)] File),
    /// Every token is taken: do not multiplex.
    Full,
}

static CLAIMS: Mutex<Option<HashMap<PathBuf, Claim>>> = Mutex::new(None);

/// A short stable id of the destination (name, address, port, user).
pub fn host_id(target: &Target) -> String {
    let mut h = Sha256::new();
    for part in [
        target.name.as_str(),
        target.address.as_str(),
        &target.port.to_string(),
        target.user.as_deref().unwrap_or(""),
    ] {
        h.update(part.as_bytes());
        h.update([0]);
    }
    h.finalize()[..8].iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

/// Try to take one token of `slot` for `id`.
fn take_token(dir: &Path, id: &str, slot: usize) -> Option<File> {
    (0..PER_SLOT).find_map(|k| {
        let path = dir.join(format!("{id}-{slot}.{k}.lock"));
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
            .ok()?;
        file.try_lock().ok().map(|()| file)
    })
}

/// The control socket this process should use for `target`, or `None` when
/// every master of the host is fully claimed (connect without multiplexing).
/// The choice is made once per process and host; a stale socket is removed.
pub fn socket(dir: &Path, target: &Target) -> Option<PathBuf> {
    let id = host_id(target);
    let key = dir.join(&id);
    let mut guard = CLAIMS.lock().unwrap_or_else(PoisonError::into_inner);
    let claims = guard.get_or_insert_with(HashMap::new);
    let claim = claims.entry(key).or_insert_with(|| {
        for slot in 0..SLOTS {
            if let Some(file) = take_token(dir, &id, slot) {
                tracing::debug!(host = %target.name, slot, "ssh master slot claimed");
                return Claim::Slot(slot, file);
            }
        }
        tracing::info!(host = %target.name, "every ssh master slot is busy; connecting without multiplexing");
        Claim::Full
    });
    let Claim::Slot(slot, _) = claim else {
        return None;
    };
    let sock = dir.join(format!("{id}-{slot}.sock"));
    remove_if_stale(&sock);
    Some(sock)
}

/// Remove a control socket nobody listens on any more.
pub fn remove_if_stale(sock: &Path) {
    #[cfg(unix)]
    if sock.exists()
        && let Err(e) = std::os::unix::net::UnixStream::connect(sock)
        && e.kind() == std::io::ErrorKind::ConnectionRefused
    {
        tracing::info!(sock = %sock.display(), "removing stale ssh control socket");
        let _ = std::fs::remove_file(sock);
    }
    #[cfg(not(unix))]
    let _ = sock;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(name: &str) -> Target {
        Target {
            name: name.to_owned(),
            address: "192.0.2.7".to_owned(),
            port: 22,
            user: None,
            identity: None,
        }
    }

    // frob:tests crates/goway/src/ssh/mux.rs::take_token
    #[test]
    fn tokens_run_out_after_slots_times_per_slot() {
        let tmp = tempfile::tempdir().unwrap();
        let held: Vec<File> = (0..SLOTS * PER_SLOT)
            .map(|i| take_token(tmp.path(), "h", i / PER_SLOT).expect("token"))
            .collect();
        assert_eq!(held.len(), SLOTS * PER_SLOT);
        assert!(take_token(tmp.path(), "h", 0).is_none());
        drop(held);
        assert!(take_token(tmp.path(), "h", 0).is_some());
    }

    // frob:tests crates/goway/src/ssh/mux.rs::host_id
    #[test]
    fn host_ids_differ_by_destination_and_are_short() {
        let a = host_id(&target("helios"));
        assert_eq!(a.len(), 16);
        assert_eq!(a, host_id(&target("helios")));
        assert_ne!(a, host_id(&target("orion")));
    }

    // frob:tests crates/goway/src/ssh/mux.rs::remove_if_stale
    #[cfg(unix)]
    #[test]
    fn a_socket_with_no_master_is_removed_and_a_live_one_kept() {
        let tmp = tempfile::tempdir().unwrap();
        let stale = tmp.path().join("stale.sock");
        drop(std::os::unix::net::UnixListener::bind(&stale).unwrap());
        assert!(stale.exists());
        remove_if_stale(&stale);
        assert!(!stale.exists());
        let live = tmp.path().join("live.sock");
        let _listener = std::os::unix::net::UnixListener::bind(&live).unwrap();
        remove_if_stale(&live);
        assert!(live.exists());
    }
}
