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
if [ "$port" = "${FAKE_DEAD_PORT:-none}" ]; then
  echo "ssh: connect to host box port $port: Connection refused" >&2
  exit 255
fi
if [ "$port" = "${FAKE_WINDOWS_PORT:-none}" ]; then
  case "$1" in
    powershell*) printf 'Windows\n%s\nAMD64\n' "$FAKE_HOSTNAME"; exit 0 ;;
  esac
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
    add_on(w, hostname, all, "2222", "none")
}

/// `host add` against a fake machine whose Windows sshd is on `windows_port`
/// and whose dead port (nothing listening) is `dead_port`.
fn add_on(w: &World, hostname: &str, all: &[&str], windows_port: &str, dead_port: &str) -> Output {
    w.goway(all)
        .env("FAKE_HOSTNAME", hostname)
        .env("FAKE_WINDOWS_PORT", windows_port)
        .env("FAKE_DEAD_PORT", dead_port)
        .env(
            "GOWAY_SSH_PASS_ENV",
            "FAKE_HOSTNAME,FAKE_WINDOWS_PORT,FAKE_DEAD_PORT,GOWAY_WINDOWS_LOOKUP",
        )
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

/// A machine with only a Windows OpenSSH server: detected, recorded as a
/// Windows host (default port 22, so no `port` line), key pinned.
// frob:tests crates/goway/src/hosts.rs::add
#[test]
fn host_add_detects_a_windows_machine_and_records_its_kind() {
    let w = empty_world();
    let fp = fake_fingerprint();
    let out = add_on(
        &w,
        "Box",
        &[
            "host",
            "add",
            "--fingerprint",
            &fp,
            "box",
            "--address",
            "box-at-home",
        ],
        "22",
        "2222",
    );
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(out.status.success(), "{stderr}");
    assert!(
        stderr.contains("added box (Windows x86_64, hostname Box)"),
        "{stderr}"
    );
    let config = std::fs::read_to_string(w.config.join("config.toml")).unwrap();
    assert!(config.contains("os = \"windows\""), "{config}");
    assert!(
        !config.contains("port ="),
        "Windows OpenSSH's 22 is the default: {config}"
    );
    let kh = std::fs::read_to_string(w.config.join("known_hosts")).unwrap();
    assert!(kh.starts_with("goway-box ssh-ed25519 "), "{kh}");
}

/// A machine with WSL on 2222 and Windows OpenSSH on 22: `host add` records
/// the WSL host (Linux), never the Windows side of the same machine.
#[test]
fn host_add_never_takes_the_windows_side_for_the_wsl_host() {
    let w = empty_world();
    let fp = fake_fingerprint();
    let out = add_on(
        &w,
        "Box",
        &[
            "host",
            "add",
            "--fingerprint",
            &fp,
            "box",
            "--address",
            "box-at-home",
        ],
        "22",
        "none",
    );
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(out.status.success(), "{stderr}");
    assert!(stderr.contains("added box (Linux x86_64"), "{stderr}");
    let config = std::fs::read_to_string(w.config.join("config.toml")).unwrap();
    assert!(!config.contains("windows"), "{config}");
}

/// A fake ssh whose key login is always refused, logging each connection.
const DENYING_SSH: &str = r#"#!/bin/sh
echo x >>"$FAKE_LOG"
echo "box: Permission denied (publickey)." >&2
exit 255
"#;

// frob:tests crates/goway/src/ssh/attempts.rs::Ledger
// frob:tests crates/goway/src/resolve.rs::SshProber
#[test]
fn repeated_refused_logins_stop_after_two_and_say_why() {
    let w = world_with_ssh(DENYING_SSH);
    std::fs::write(w.config.join("config.toml"), "# my pool\n").unwrap();
    let log = w.root.join("ssh.log");
    let try_add = || {
        w.goway(&[
            "host",
            "add",
            "box",
            "--address",
            "192.0.2.7",
            "--port",
            "22",
        ])
        .env("FAKE_LOG", &log)
        .env("GOWAY_SSH_PASS_ENV", "FAKE_LOG,GOWAY_WINDOWS_LOOKUP")
        .output()
        .unwrap()
    };
    let _ = try_add();
    let _ = try_add();
    let third = try_add();
    let stderr = String::from_utf8_lossy(&third.stderr).into_owned();
    assert!(stderr.contains("holding back"), "{stderr}");
    let calls = std::fs::read_to_string(&log).unwrap().lines().count();
    assert_eq!(calls, 2, "no third login was attempted");
}

/// Denies the first login, then refuses connections like a banning firewall.
const BANNING_SSH: &str = r#"#!/bin/sh
if [ -s "$FAKE_LOG" ]; then
  echo "ssh: connect to host box port 22: Connection refused" >&2
  exit 255
fi
echo x >>"$FAKE_LOG"
echo "box: Permission denied (publickey)." >&2
exit 255
"#;

// frob:tests crates/goway/src/resolve.rs::SshProber
#[test]
fn a_refusal_right_after_failed_logins_is_reported_as_a_probable_ban() {
    let w = world_with_ssh(BANNING_SSH);
    std::fs::write(w.config.join("config.toml"), "# my pool\n").unwrap();
    let log = w.root.join("ssh.log");
    let try_add = || {
        w.goway(&[
            "host",
            "add",
            "box",
            "--address",
            "192.0.2.7",
            "--port",
            "22",
        ])
        .env("FAKE_LOG", &log)
        .env("GOWAY_SSH_PASS_ENV", "FAKE_LOG,GOWAY_WINDOWS_LOOKUP")
        .output()
        .unwrap()
    };
    let _ = try_add();
    let second = try_add();
    let stderr = String::from_utf8_lossy(&second.stderr).into_owned();
    assert!(stderr.contains("probably banned"), "{stderr}");
    assert!(
        stderr.contains("fail2ban-client set sshd unbanip"),
        "{stderr}"
    );
}
