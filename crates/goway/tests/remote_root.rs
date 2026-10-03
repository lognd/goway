//! The remote script never adopts or wipes a directory that is not goway's.
#![cfg(unix)]

use std::path::Path;
use std::process::{Command, Output};

const SCRIPT: &str = include_str!("../src/remote.sh");

fn remote(home: &Path, verb: &str, args: &[&str]) -> Output {
    Command::new("bash")
        .args(["-c", SCRIPT, "goway", verb])
        .args(args)
        .env("HOME", home)
        .output()
        .unwrap()
}

#[test]
fn receive_refuses_a_foreign_directory() {
    let home = tempfile::tempdir().unwrap();
    let root = home.path().join(".ssh");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("authorized_keys"), "key").unwrap();
    let out = remote(home.path(), "manifest", &[".ssh", "abc"]);
    assert_eq!(out.status.code(), Some(125), "{out:?}");
    assert!(String::from_utf8_lossy(&out.stderr).contains("not goway state"));
    assert!(!root.join(".goway-root").exists());
    assert!(root.join("authorized_keys").exists());
}

#[test]
fn purge_keeps_foreign_files_in_a_marked_root() {
    let home = tempfile::tempdir().unwrap();
    let root = home.path().join(".cache/goway");
    let out = remote(home.path(), "manifest", &[".cache/goway", "abc"]);
    assert!(out.status.success(), "{out:?}");
    assert!(root.join(".goway-root").exists());
    std::fs::write(root.join("precious"), "x").unwrap();
    let out = remote(home.path(), "purge", &[".cache/goway"]);
    assert!(out.status.success(), "{out:?}");
    assert!(root.join("precious").exists(), "foreign file removed");
    assert!(!root.join("seed").exists());
    assert!(!root.join(".goway-root").exists());

    // With nothing foreign left, the root itself goes too.
    std::fs::remove_file(root.join("precious")).unwrap();
    let out = remote(home.path(), "manifest", &[".cache/goway", "abc"]);
    assert!(out.status.success(), "{out:?}");
    assert!(
        remote(home.path(), "purge", &[".cache/goway"])
            .status
            .success()
    );
    assert!(!root.exists());
}
