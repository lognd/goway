//! `goway ssh setup` end to end against a fake sshd: batch (key-only)
//! logins are refused unless the offered goway key is in the "remote"
//! `~/.ssh/authorized_keys`; password logins (BatchMode=no) are accepted.
#![cfg(target_os = "linux")]

mod common;

use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;
use std::process::Output;

use common::{World, world_with_ssh};

const AUTH_SSH: &str = r#"#!/bin/sh
batch=no; idf=""; kh=""; alias=""; accept=no
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
    -p|-l) shift 2 ;;
    -t) shift ;;
    --) shift; shift; break ;;
    *) shift ;;
  esac
done
if [ "$accept" = yes ] && [ -n "$kh" ] && ! grep -q "^$alias " "$kh" 2>/dev/null; then
  echo "$alias ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl" >>"$kh"
fi
if [ "$batch" = no ]; then : >"$HOME/../password-login-happened"; fi
if [ "$batch" = yes ]; then
  if [ -z "$idf" ] || [ ! -f "$idf.pub" ] || ! grep -qF "$(cut -d' ' -f2 "$idf.pub")" "$HOME/.ssh/authorized_keys" 2>/dev/null; then
    echo "tester@127.0.0.1: Permission denied (publickey)." >&2
    exit 255
  fi
fi
exec sh -c "$1"
"#;

struct Setup {
    w: World,
    home: std::path::PathBuf,
}

fn setup_world() -> Setup {
    let w = world_with_ssh(AUTH_SSH);
    let home = w.root.join("home");
    std::fs::create_dir(&home).unwrap();
    Setup { w, home }
}

impl Setup {
    fn run(&self, args: &[&str]) -> Output {
        self.w
            .goway(args)
            .env("HOME", &self.home)
            .env_remove("SSH_AUTH_SOCK")
            .output()
            .unwrap()
    }
}

/// Every path under `dir` with its mode and content.
fn snapshot(dir: &Path) -> BTreeMap<String, (u32, Vec<u8>)> {
    let mut out = BTreeMap::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap() {
            let p = e.unwrap().path();
            let meta = std::fs::symlink_metadata(&p).unwrap();
            let rel = p.strip_prefix(dir).unwrap().display().to_string();
            if meta.is_dir() {
                stack.push(p.clone());
                out.insert(rel, (meta.permissions().mode(), Vec::new()));
            } else {
                out.insert(rel, (meta.permissions().mode(), std::fs::read(&p).unwrap()));
            }
        }
    }
    out
}

fn mode(p: &Path) -> u32 {
    std::fs::metadata(p).unwrap().permissions().mode() & 0o777
}

// frob:tests crates/goway/src/sshsetup.rs::setup
// frob:tests crates/goway/src/remotesys.rs::RemoteSystem.output
#[test]
fn setup_authorizes_a_key_with_one_password_login_and_undo_restores_everything() {
    let s = setup_world();
    let before = (snapshot(&s.home), snapshot(&s.w.config));

    let denied = s.run(&["run", "--", "true"]);
    assert_eq!(
        denied.status.code(),
        Some(125),
        "key login fails before setup"
    );

    let out = s.run(&["ssh", "setup", "local"]);
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(out.status.success(), "{stderr}");
    assert!(stderr.contains("key login to local works"), "{stderr}");
    assert!(
        s.w.config.join("id_ed25519").is_file(),
        "goway's own key, not ~/.ssh"
    );
    let ak = s.home.join(".ssh/authorized_keys");
    let line = std::fs::read_to_string(&ak).unwrap();
    assert!(
        line.starts_with("no-agent-forwarding,no-port-forwarding,no-X11-forwarding ssh-ed25519 ")
            && line.contains(" goway:"),
        "{line}"
    );
    assert_eq!((mode(&s.home.join(".ssh")), mode(&ak)), (0o700, 0o600));
    let config = std::fs::read_to_string(s.w.config.join("config.toml")).unwrap();
    assert!(config.contains("identity = "), "{config}");

    let ok = s.run(&["run", "--", "true"]);
    assert_eq!(
        ok.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&ok.stderr)
    );

    let again = s.run(&["ssh", "setup", "local"]);
    assert!(
        !again.status.success(),
        "a second setup is refused until --undo"
    );

    let undo = s.run(&["ssh", "setup", "local", "--undo"]);
    assert!(
        undo.status.success(),
        "{}",
        String::from_utf8_lossy(&undo.stderr)
    );
    assert_eq!((snapshot(&s.home), snapshot(&s.w.config)), before);
}

#[test]
fn existing_ssh_dir_and_keys_are_restored_exactly_by_undo() {
    let s = setup_world();
    let ssh_dir = s.home.join(".ssh");
    std::fs::create_dir(&ssh_dir).unwrap();
    std::fs::set_permissions(&ssh_dir, std::fs::Permissions::from_mode(0o755)).unwrap();
    let ak = ssh_dir.join("authorized_keys");
    std::fs::write(&ak, "ssh-ed25519 AAAAother someone@else\n").unwrap();
    std::fs::set_permissions(&ak, std::fs::Permissions::from_mode(0o644)).unwrap();
    let before = (snapshot(&s.home), snapshot(&s.w.config));

    let out = s.run(&["ssh", "setup", "local"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = std::fs::read_to_string(&ak).unwrap();
    assert!(
        text.starts_with("ssh-ed25519 AAAAother someone@else\n"),
        "{text}"
    );
    assert_eq!(
        (mode(&ssh_dir), mode(&ak)),
        (0o700, 0o600),
        "modes tightened"
    );

    let undo = s.run(&["ssh", "setup", "local", "--undo"]);
    assert!(
        undo.status.success(),
        "{}",
        String::from_utf8_lossy(&undo.stderr)
    );
    assert_eq!(
        (snapshot(&s.home), snapshot(&s.w.config)),
        before,
        "loose modes and content back"
    );
}

fn fake_fingerprint() -> String {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("k");
    std::fs::write(
        &file,
        "x ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl\n",
    )
    .unwrap();
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

#[test]
fn a_new_host_is_confirmed_before_any_password_and_pinned_once() {
    let s = setup_world();
    let unconfirmed = s.run(&["ssh", "setup", "newbox", "--address", "127.0.0.1"]);
    assert_eq!(unconfirmed.status.code(), Some(125));
    assert!(
        !s.w.root.join("password-login-happened").exists(),
        "no password login before the key is confirmed"
    );
    assert!(!s.home.join(".ssh").exists());

    let fp = fake_fingerprint();
    let out = s.run(&[
        "ssh",
        "setup",
        "newbox",
        "--address",
        "127.0.0.1",
        "--fingerprint",
        &fp,
    ]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let kh = std::fs::read_to_string(s.w.config.join("known_hosts")).unwrap();
    assert_eq!(kh.matches("goway-newbox ").count(), 1, "{kh}");
    let config = std::fs::read_to_string(s.w.config.join("config.toml")).unwrap();
    assert!(config.contains("name = \"newbox\""), "{config}");
    let undo = s.run(&["ssh", "setup", "newbox", "--undo"]);
    assert!(
        undo.status.success(),
        "{}",
        String::from_utf8_lossy(&undo.stderr)
    );
    assert!(
        !std::fs::read_to_string(s.w.config.join("config.toml"))
            .unwrap()
            .contains("newbox")
    );
}

// frob:tests crates/goway/src/add.rs::add
#[test]
fn add_registers_a_new_helper_in_one_command_and_is_idempotent() {
    let s = setup_world();
    let fp = fake_fingerprint();
    let out = s.run(&[
        "add",
        "newbox",
        "--address",
        "127.0.0.1",
        "--fingerprint",
        &fp,
    ]);
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(stderr.contains("key login to newbox works"), "{stderr}");
    assert!(stderr.contains("goway: next:"), "{stderr}");
    let ran = s.run(&["run", "--host", "newbox", "--", "true"]);
    assert_eq!(
        ran.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&ran.stderr)
    );

    let before = (snapshot(&s.home), snapshot(&s.w.config));
    let again = s.run(&[
        "add",
        "newbox",
        "--address",
        "127.0.0.1",
        "--fingerprint",
        &fp,
    ]);
    let stderr = String::from_utf8_lossy(&again.stderr).into_owned();
    assert!(stderr.contains("set up before"), "{stderr}");
    let after = (snapshot(&s.home), snapshot(&s.w.config));
    assert_eq!(before.0, after.0, "the helper is not touched again");
    assert_eq!(
        before.1.get("config.toml"),
        after.1.get("config.toml"),
        "the config is not touched again"
    );
}

// frob:tests crates/goway/src/uninstall.rs::uninstall
#[test]
fn uninstall_everywhere_removes_goway_from_helper_and_laptop() {
    let s = setup_world();
    // A pool with no hosts yet, as a newcomer starts.
    let remote = s.w.remote.display().to_string();
    std::fs::write(
        s.w.config.join("config.toml"),
        format!("[defaults]\nremote_root = \"{remote}\"\n"),
    )
    .unwrap();
    let home_before = snapshot(&s.home);
    let fp = fake_fingerprint();
    let added = s.run(&[
        "add",
        "newbox",
        "--address",
        "127.0.0.1",
        "--fingerprint",
        &fp,
    ]);
    assert!(
        added.status.success(),
        "{}",
        String::from_utf8_lossy(&added.stderr)
    );
    assert!(
        s.run(&["run", "--host", "newbox", "--", "true"])
            .status
            .success()
    );
    assert!(s.w.remote.join(".goway-root").exists());

    // Without --everywhere and without a terminal: list, remove nothing.
    let asked = s.run(&["uninstall"]);
    let listing = String::from_utf8_lossy(&asked.stdout).into_owned();
    assert!(
        listing.contains("on newbox:") && listing.contains("on this laptop:"),
        "{listing}"
    );
    assert!(String::from_utf8_lossy(&asked.stderr).contains("nothing was removed"));
    assert!(s.w.remote.exists());

    let out = s.run(&["uninstall", "--everywhere"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!s.w.remote.exists(), "goway's remote state is gone");
    // The fake helper shares this laptop's PATH, so the doctor's version
    // probe runs the local rustup with an empty home and it creates
    // ~/.rustup; a real helper has its own toolchain. Not goway's change.
    let mut home_after = snapshot(&s.home);
    home_after.retain(|k, _| !k.starts_with(".rustup"));
    assert_eq!(home_after, home_before, "the helper's ~/.ssh is as before");
    let left: Vec<String> = std::fs::read_dir(&s.w.config)
        .map(|d| {
            d.map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    assert!(
        left.is_empty(),
        "goway files left in its config dir: {left:?}"
    );
}
