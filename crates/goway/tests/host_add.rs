//! `goway host add` end to end against a fake sshd that pins keys like
//! OpenSSH (`StrictHostKeyChecking=accept-new` writes the alias line) and
//! can answer as Windows OpenSSH on one port.
#![cfg(unix)]

mod common;

use std::process::Output;

use common::{World, world_with_ssh};

const SSHD: &str = r#"#!/bin/sh
port=22; kh=""; alias=""; accept=no
while [ $# -gt 0 ]; do
  case "$1" in
    -G) echo "user tester"; exit 0 ;;
    -o) case "$2" in
          UserKnownHostsFile=*) kh=${2#UserKnownHostsFile=} ;;
          HostKeyAlias=*) alias=${2#HostKeyAlias=} ;;
          StrictHostKeyChecking=accept-new) accept=yes ;;
        esac; shift 2 ;;
    -p) port=$2; shift 2 ;;
    -l) shift 2 ;;
    --) shift; shift; break ;;
    *) shift ;;
  esac
done
if [ "$accept" = yes ] && [ -n "$kh" ] && ! grep -q "^$alias " "$kh" 2>/dev/null; then
  echo "$alias ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl" >>"$kh"
fi
if [ "$port" = "${FAKE_WINDOWS_PORT:-none}" ]; then
  echo "'uname' is not recognized as an internal or external command," >&2
  exit 1
fi
case "$1" in
  "uname -s; uname -n; uname -m") printf 'Linux\n%s\nx86_64\n' "$FAKE_HOSTNAME"; exit 0 ;;
esac
exec sh -c "$1"
"#;

const FAKE_KEY: &str = "AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl";

/// The SHA256 fingerprint of the fake sshd's host key, via ssh-keygen.
fn fake_fingerprint() -> String {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("k");
    std::fs::write(&file, format!("x ssh-ed25519 {FAKE_KEY}\n")).unwrap();
    let out = std::process::Command::new("ssh-keygen")
        .arg("-lf")
        .arg(&file)
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .find(|w| w.starts_with("SHA256:"))
        .unwrap()
        .to_owned()
}

/// `host add` with the fake host's real fingerprint.
fn add(w: &World, hostname: &str, args: &[&str]) -> Output {
    let fp = fake_fingerprint();
    let mut all = vec!["host", "add", "--fingerprint", fp.as_str()];
    all.extend_from_slice(args);
    add_raw(w, hostname, &all)
}

fn add_raw(w: &World, hostname: &str, all: &[&str]) -> Output {
    w.goway(all)
        .env("FAKE_HOSTNAME", hostname)
        .env("FAKE_WINDOWS_PORT", "2222")
        .output()
        .unwrap()
}

fn empty_world() -> World {
    let w = world_with_ssh(SSHD);
    std::fs::write(w.config.join("config.toml"), "# my pool\n").unwrap();
    w
}

// frob:tests crates/goway/src/hosts.rs::add
#[test]
fn host_add_skips_windows_sshd_and_pins_the_linux_machine() {
    let w = empty_world();
    let out = add(&w, "Box", &["box", "--address", "box-at-home"]);
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(out.status.success(), "{stderr}");
    assert!(stderr.contains("looking for box on port 2222"), "{stderr}");
    assert!(
        stderr.contains("added box (Linux x86_64, hostname Box)"),
        "{stderr}"
    );
    let config = std::fs::read_to_string(w.config.join("config.toml")).unwrap();
    assert!(config.starts_with("# my pool\n"), "{config}");
    assert!(
        config.contains("name = \"box\"") && config.contains("port = 22"),
        "{config}"
    );
    let kh = std::fs::read_to_string(w.config.join("known_hosts")).unwrap();
    assert!(kh.starts_with("goway-box ssh-ed25519 "), "{kh}");
    let list = String::from_utf8_lossy(&w.run(&["host", "list"]).stdout).into_owned();
    assert!(list.contains("box"), "{list}");
}

#[test]
fn host_add_refuses_a_machine_with_another_hostname() {
    let w = empty_world();
    let out = add(
        &w,
        "someone-else",
        &["box", "--address", "box-at-home", "--port", "22"],
    );
    assert_eq!(out.status.code(), Some(125));
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(stderr.contains("is `someone-else`, not `box`"), "{stderr}");
    assert_eq!(
        std::fs::read_to_string(w.config.join("config.toml")).unwrap(),
        "# my pool\n"
    );
    let kh = std::fs::read_to_string(w.config.join("known_hosts")).unwrap_or_default();
    assert!(
        !kh.contains("goway-box"),
        "no key pinned for the wrong machine: {kh}"
    );
}

// frob:tests crates/goway/src/hosts.rs::confirm_key
// frob:tests crates/goway/src/hosts.rs::fingerprints
#[test]
fn nothing_is_pinned_without_a_confirmed_fingerprint() {
    let w = empty_world();
    // No terminal and no --fingerprint: refuse, pin nothing.
    let out = add_raw(
        &w,
        "box",
        &[
            "host",
            "add",
            "box",
            "--address",
            "box-at-home",
            "--port",
            "22",
        ],
    );
    assert_eq!(out.status.code(), Some(125));
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(
        stderr.contains("needs confirmation") && stderr.contains("--fingerprint"),
        "{stderr}"
    );
    // A wrong fingerprint (an impostor answering for the name): refuse.
    let wrong = add_raw(
        &w,
        "box",
        &[
            "host",
            "add",
            "box",
            "--address",
            "box-at-home",
            "--port",
            "22",
            "--fingerprint",
            "SHA256:AAAAnotthekey",
        ],
    );
    assert_eq!(wrong.status.code(), Some(125));
    assert!(String::from_utf8_lossy(&wrong.stderr).contains("not the machine you named"));
    let kh = std::fs::read_to_string(w.config.join("known_hosts")).unwrap_or_default();
    assert!(!kh.contains("goway-box"), "{kh}");
    assert_eq!(
        std::fs::read_to_string(w.config.join("config.toml")).unwrap(),
        "# my pool\n"
    );
}

#[test]
fn a_bad_name_touches_nothing() {
    let w = empty_world();
    let out = add_raw(&w, "x", &["host", "add", "bad*", "--address", "127.0.0.1"]);
    assert_eq!(
        out.status.code(),
        Some(2),
        "rejected by the argument parser"
    );
    assert!(!w.config.join("known_hosts").exists());
    let out = w.run(&["ssh", "setup", "../x"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(
        std::fs::read_dir(&w.config).unwrap().count() == 1,
        "only config.toml"
    );
}
