//! ssh multiplexing under a wave of runs: a master per claimed slot, stale
//! sockets replaced, never the shared `%C` path that one master's
//! `MaxSessions` would overrun.
#![cfg(unix)]

use std::path::PathBuf;

use goway::ssh::{self, KeyPolicy, Settings, Target};

fn target() -> Target {
    Target {
        name: "helios".to_owned(),
        address: "192.0.2.7".to_owned(),
        port: 22,
        user: None,
        identity: None,
    }
}

fn control_path(dir: &std::path::Path) -> String {
    let settings = Settings {
        known_hosts: dir.join("known_hosts"),
        control_dir: Some(dir.to_owned()),
        connect_timeout_secs: 5,
    };
    let cmd = ssh::command(&target(), &settings, KeyPolicy::Strict, "true");
    cmd.get_args()
        .map(|a| a.to_string_lossy().into_owned())
        .find_map(|a| a.strip_prefix("ControlPath=").map(str::to_owned))
        .expect("a ControlPath option")
}

// frob:tests crates/goway/src/ssh.rs::command
#[test]
fn a_run_claims_its_own_master_slot_not_the_shared_hash_path() {
    let tmp = tempfile::tempdir().unwrap();
    let path = control_path(tmp.path());
    assert!(!path.contains("%C"), "{path}");
    assert!(path.ends_with("-0.sock"), "{path}");
    // Asked again, the same process keeps its slot.
    assert_eq!(path, control_path(tmp.path()));
}

// frob:tests crates/goway/src/ssh/mux.rs::remove_if_stale
#[test]
fn a_stale_control_socket_is_replaced_instead_of_disabling_multiplexing() {
    let tmp = tempfile::tempdir().unwrap();
    let first = PathBuf::from(control_path(tmp.path()));
    drop(std::os::unix::net::UnixListener::bind(&first).unwrap());
    assert!(first.exists());
    let again = PathBuf::from(control_path(tmp.path()));
    assert_eq!(first, again);
    assert!(!again.exists(), "the stale socket should be removed");
}

// frob:tests crates/goway/src/ssh/mux.rs::socket
#[test]
fn a_ninth_process_over_the_limit_connects_without_multiplexing() {
    use goway::ssh::mux::{PER_SLOT, SLOTS};
    let tmp = tempfile::tempdir().unwrap();
    // Take every token as other goway processes would (advisory locks).
    let id = ssh::mux::host_id(&target());
    let held: Vec<std::fs::File> = (0..SLOTS)
        .flat_map(|slot| (0..PER_SLOT).map(move |k| (slot, k)))
        .map(|(slot, k)| {
            let f = std::fs::OpenOptions::new()
                .create(true)
                .truncate(false)
                .write(true)
                .open(tmp.path().join(format!("{id}-{slot}.{k}.lock")))
                .unwrap();
            f.try_lock().unwrap();
            f
        })
        .collect();
    let path = control_args(tmp.path());
    assert!(path.contains(&"ControlPath=none".to_owned()), "{path:?}");
    drop(held);
}

fn control_args(dir: &std::path::Path) -> Vec<String> {
    let settings = Settings {
        known_hosts: dir.join("known_hosts"),
        control_dir: Some(dir.to_owned()),
        connect_timeout_secs: 5,
    };
    ssh::command(&target(), &settings, KeyPolicy::Strict, "true")
        .get_args()
        .map(|a| a.to_string_lossy().into_owned())
        .collect()
}
