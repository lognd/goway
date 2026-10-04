//! `goway ssh setup HOST --rsudo` on a native Windows host: the administrator step runs through
//! the host's Windows ssh as the given administrator account (a fake sshd that "runs" the encoded
//! PowerShell by authorizing the key it names), never with a password, and without it goway
//! stops with the exact command to run as administrator.
#![cfg(target_os = "linux")]

mod common;

use std::process::Output;

const WIN_SSH: &str = r#"#!/bin/sh
batch=no; idf=""; kh=""; alias=""; accept=no; user=""
while [ $# -gt 0 ]; do
  case "$1" in
    -G) echo "user tester"; exit 0 ;;
    -o) case "$2" in
          BatchMode=yes) batch=yes ;;
          IdentityFile=*) idf=${2#IdentityFile=} ;;
          UserKnownHostsFile=*) kh=${2#UserKnownHostsFile=} ;;
          HostKeyAlias=*) alias=${2#HostKeyAlias=} ;;
          StrictHostKeyChecking=accept-new) accept=yes ;;
        esac; shift 2 ;;
    -l) user=$2; shift 2 ;;
    -p) shift 2 ;;
    -t) shift ;;
    --) shift; shift; break ;;
    *) shift ;;
  esac
done
if [ "$accept" = yes ] && [ -n "$kh" ] && ! grep -q "^$alias " "$kh" 2>/dev/null; then
  echo "$alias ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl" >>"$kh"
fi
echo "$user $*" >>"$HOME/../ssh-calls"
if [ "$user" = admin ]; then
  # The administrator account: its key works; "run" the encoded PowerShell by authorizing the
  # key the goway-setup command names.
  blob=${1##* }
  text=$(printf %s "$blob" | base64 -d | iconv -f UTF-16LE -t UTF-8)
  case "$text" in *"Invoke-GowaySetup @('install', '--host', '--native', '--authorized-key', '"*) ;; *) echo "unexpected step: $text" >&2; exit 9 ;; esac
  case "$text" in *Test-GowayProtectedPath*) ;; *) echo "step does not verify goway-setup: $text" >&2; exit 9 ;; esac
  key=$(printf %s "$text" | sed -n "s/.*'--authorized-key', '\([^']*\)'.*/\1/p")
  mkdir -p "$HOME/.ssh"; echo "$key" >>"$HOME/.ssh/authorized_keys"
  : >"$HOME/../admin-step-ran"
  exit 0
fi
if [ "$batch" = yes ]; then
  if [ -z "$idf" ] || [ ! -f "$idf.pub" ] || ! grep -qF "$(cut -d' ' -f2 "$idf.pub")" "$HOME/.ssh/authorized_keys" 2>/dev/null; then
    echo "tester@127.0.0.1: Permission denied (publickey)." >&2
    exit 255
  fi
fi
exec sh -c "$1"
"#;

struct Win {
    w: common::World,
    home: std::path::PathBuf,
}

fn win_world() -> Win {
    let w = common::world_with_ssh(WIN_SSH);
    let home = w.root.join("home");
    std::fs::create_dir(&home).unwrap();
    let mut config = std::fs::read_to_string(w.config.join("config.toml")).unwrap();
    config.push_str("\n[[host]]\nname = \"winbox\"\naddress = \"127.0.0.1\"\nos = \"windows\"\n");
    std::fs::write(w.config.join("config.toml"), config).unwrap();
    Win { w, home }
}

impl Win {
    fn run(&self, args: &[&str]) -> Output {
        self.w
            .goway(args)
            .env("HOME", &self.home)
            .env_remove("SSH_AUTH_SOCK")
            .output()
            .unwrap()
    }
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

// frob:tests crates/goway/src/winadmin.rs::elevate
// frob:tests crates/goway/src/sshsetup.rs::native_setup_command
#[test]
fn rsudo_authorizes_the_key_through_the_windows_ssh_admin_account_without_a_password() {
    let s = win_world();
    let out = s.run(&[
        "ssh",
        "setup",
        "winbox",
        "--rsudo",
        "--windows-admin",
        "admin",
        "--yes",
    ]);
    let err = text(&out.stderr);
    assert!(out.status.success(), "{err}");
    assert!(s.w.root.join("admin-step-ran").exists(), "{err}");
    assert!(err.contains("Key works"), "{err}");
    let calls = std::fs::read_to_string(s.w.root.join("ssh-calls")).unwrap();
    assert_eq!(
        calls.lines().filter(|l| l.starts_with("admin ")).count(),
        1,
        "one administrator login only: {calls}"
    );
    assert!(!calls.contains("BatchMode=no"), "{calls}");
}

// frob:tests crates/goway/src/winadmin.rs::elevate
#[test]
fn without_an_admin_route_it_prints_the_exact_command_and_what_was_tried() {
    let s = win_world();
    let out = s.run(&["ssh", "setup", "winbox", "--rsudo", "--yes"]);
    let err = text(&out.stderr);
    assert!(!out.status.success(), "{err}");
    assert!(err.contains("not elevated"), "{err}");
    assert!(err.contains("no administrator ssh account"), "{err}");
    let all = format!("{err}{}", text(&out.stdout));
    assert!(
        all.contains("goway-setup install --host --native --authorized-key"),
        "{all}"
    );
    assert!(!s.w.root.join("admin-step-ran").exists());
}
