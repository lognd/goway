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
if [ "$batch" = no ] && [ -f "$HOME/../no-password" ]; then
  echo "tester@127.0.0.1: Permission denied (publickey,password)." >&2
  exit 255
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

/// The commands goway printed for the user to run on the helper, one per line.
fn printed_commands(text: &str) -> String {
    text.lines()
        .map(|l| l.trim_end_matches('\r'))
        .filter(|l| {
            l.starts_with("mkdir -p ~/.ssh")
                || l.starts_with("echo 'no-agent-forwarding")
                || l.starts_with("chmod 600 ~/.ssh")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Run the printed commands as the helper's user (the fake helper shares this machine).
fn run_on_helper(s: &Setup, commands: &str) {
    let ok = std::process::Command::new("sh")
        .arg("-c")
        .arg(commands)
        .env("HOME", &s.home)
        .status()
        .unwrap();
    assert!(ok.success(), "{commands}");
}

// frob:tests crates/goway/src/sshsetup.rs::HandInstall
// frob:tests crates/goway/src/ssh/attempts.rs::permit
#[test]
fn a_helper_that_refuses_passwords_gets_the_key_by_hand_without_a_terminal() {
    let s = setup_world();
    std::fs::write(s.w.root.join("no-password"), "").unwrap();
    let fp = fake_fingerprint();
    let args = [
        "ssh",
        "setup",
        "newbox",
        "--address",
        "127.0.0.1",
        "--user",
        "tester",
        "--fingerprint",
        &fp,
    ];
    let out = s.run(&args);
    let (stdout, stderr) = (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    );
    assert_eq!(out.status.code(), Some(125), "{stderr}");
    // A friendly turn, not a wall of text: one likely cause, a pointer to the rest.
    assert!(
        stderr.contains("That password did not work on newbox."),
        "{stderr}"
    );
    assert!(stderr.contains("docs/troubleshooting.md"), "{stderr}");
    assert!(stderr.contains("Let's do it the other way:"), "{stderr}");
    assert!(stderr.contains("rerun"), "{stderr}");
    let commands = printed_commands(&stdout);
    assert!(
        commands.starts_with("mkdir -p ~/.ssh && chmod 700 ~/.ssh\necho 'no-agent-forwarding,no-port-forwarding,no-X11-forwarding ssh-ed25519 "),
        "{stdout}"
    );
    // Only the public half is ever printed.
    let public = std::fs::read_to_string(s.w.config.join("id_ed25519.pub")).unwrap();
    let blob = public.split_whitespace().nth(1).unwrap();
    assert!(commands.contains(blob), "{commands}");
    assert!(!stdout.contains("PRIVATE KEY") && !stderr.contains("PRIVATE KEY"));
    assert!(
        !s.home.join(".ssh/authorized_keys").exists(),
        "nothing was installed"
    );
    assert!(
        !std::fs::read_to_string(s.w.config.join("config.toml"))
            .unwrap()
            .contains("newbox"),
        "the host is not added before its key works"
    );

    // The user runs the commands on the helper and reruns the same command.
    run_on_helper(&s, &commands);
    let again = s.run(&args);
    let stderr = String::from_utf8_lossy(&again.stderr).into_owned();
    assert!(again.status.success(), "{stderr}");
    // goway's own key is offered to the first probe, so the rerun costs no
    // failed login at all (and the host is registered with that key).
    assert!(stderr.contains("already works"), "{stderr}");
    let config = std::fs::read_to_string(s.w.config.join("config.toml")).unwrap();
    assert!(config.contains("name = \"newbox\""), "{config}");
    assert!(config.contains("id_ed25519"), "{config}");
}

// frob:tests crates/goway/src/sshsetup.rs::HandInstall
#[test]
fn no_password_never_tries_a_password_login() {
    let s = setup_world();
    let fp = fake_fingerprint();
    let out = s.run(&[
        "ssh",
        "setup",
        "newbox",
        "--address",
        "127.0.0.1",
        "--fingerprint",
        &fp,
        "--no-password",
    ]);
    assert_eq!(out.status.code(), Some(125));
    assert!(
        printed_commands(&String::from_utf8_lossy(&out.stdout)).contains("authorized_keys"),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(
        !s.w.root.join("password-login-happened").exists(),
        "no password login was attempted"
    );
}

/// A transcript with colors, carriage returns, tracing lines and per-run values removed, so a
/// wording change shows up as a diff and nothing else does.
fn normalize(raw: &str, s: &Setup) -> String {
    let mut text = String::new();
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' && chars.peek() == Some(&'[') {
            for d in chars.by_ref() {
                if d.is_ascii_alphabetic() {
                    break;
                }
            }
        } else if c != '\r' {
            text.push(c);
        }
    }
    let config = s.w.config.display().to_string();
    text.lines()
        .filter(|l| !l.contains(" WARN ") && !l.contains(" INFO "))
        .map(|l| {
            if l.starts_with("echo 'no-agent-forwarding") {
                "echo '<restricted key line>' >> ~/.ssh/authorized_keys".to_owned()
            } else {
                l.replace(&config, "<config>")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Run `goway ssh setup newbox ...` on a pty (so stdin is a terminal), typing each `send` once
/// its `wait` text has appeared; returns the exit status and the normalized transcript.
fn on_a_pty(s: &Setup, steps: &[(&str, &str)]) -> Option<(std::process::ExitStatus, String)> {
    use std::io::{Read as _, Write as _};
    use std::process::Stdio;
    use std::sync::{Arc, Mutex};
    if !std::path::Path::new("/usr/bin/script").exists() {
        return None; // no pty helper on this machine
    }
    let fp = fake_fingerprint();
    let line = format!(
        "{} ssh setup newbox --address 127.0.0.1 --user tester --fingerprint {fp}",
        env!("CARGO_BIN_EXE_goway")
    );
    let probe = s.w.goway(&["--version"]);
    let mut cmd = std::process::Command::new("script");
    cmd.args(["-qec", &line, "/dev/null"]);
    for (k, v) in probe.get_envs() {
        match v {
            Some(v) => cmd.env(k, v),
            None => cmd.env_remove(k),
        };
    }
    cmd.env("HOME", &s.home)
        .env("NO_COLOR", "1")
        .env_remove("SSH_AUTH_SOCK");
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut stdout = child.stdout.take().unwrap();
    let seen = Arc::new(Mutex::new(String::new()));
    let sink = Arc::clone(&seen);
    std::thread::spawn(move || {
        let mut chunk = [0u8; 4096];
        while let Ok(n) = stdout.read(&mut chunk) {
            if n == 0 {
                break;
            }
            sink.lock()
                .unwrap()
                .push_str(&String::from_utf8_lossy(&chunk[..n]));
        }
    });
    let mut from = 0;
    for (wait, send) in steps {
        common::wait_for(wait, || seen.lock().unwrap()[from..].contains(wait));
        let text = seen.lock().unwrap().clone();
        from = text.rfind(wait).unwrap() + wait.len();
        if send.starts_with("PASTE") {
            run_on_helper(s, &printed_commands(&text));
        }
        child
            .stdin
            .as_mut()
            .unwrap()
            .write_all(if send.starts_with("PASTE") {
                b"\n"
            } else {
                send.as_bytes()
            })
            .unwrap();
    }
    let status = child.wait().unwrap();
    let text = seen.lock().unwrap().clone();
    Some((status, normalize(&text, s)))
}

const ASK: &str = "Do you know the password of tester on newbox? [Y/n]";
const PRESS: &str = "Then press Enter here (Ctrl-C to stop).";

// frob:tests crates/goway/src/sshsetup.rs::knows_password
#[test]
fn golden_yes_with_a_good_password() {
    let s = setup_world();
    let Some((status, text)) = on_a_pty(&s, &[(ASK, "y\n")]) else {
        return;
    };
    assert!(status.success(), "{text}");
    assert_eq!(text, GOLDEN_GOOD_PASSWORD.trim_matches('\n'));
}

// frob:tests crates/goway/src/sshsetup.rs::HandInstall
#[test]
fn golden_yes_with_a_bad_password_then_paste() {
    let s = setup_world();
    std::fs::write(s.w.root.join("no-password"), "").unwrap();
    let Some((status, text)) = on_a_pty(&s, &[(ASK, "y\n"), (PRESS, "PASTE")]) else {
        return;
    };
    assert!(status.success(), "{text}");
    assert_eq!(text, GOLDEN_BAD_PASSWORD.trim_matches('\n'));
}

// frob:tests crates/goway/src/sshsetup.rs::knows_password
#[test]
fn golden_no_then_paste_never_tries_a_password() {
    let s = setup_world();
    let Some((status, text)) = on_a_pty(&s, &[(ASK, "n\n"), (PRESS, "PASTE")]) else {
        return;
    };
    assert!(status.success(), "{text}");
    assert!(!s.w.root.join("password-login-happened").exists());
    assert_eq!(text, GOLDEN_NO_PASSWORD.trim_matches('\n'));
}

// frob:tests crates/goway/src/sshsetup.rs::HandInstall
#[test]
fn golden_no_terminal_prints_the_paste_block_and_stops() {
    let s = setup_world();
    std::fs::write(s.w.root.join("no-password"), "").unwrap();
    let fp = fake_fingerprint();
    let out = s.run(&[
        "ssh",
        "setup",
        "newbox",
        "--address",
        "127.0.0.1",
        "--user",
        "tester",
        "--fingerprint",
        &fp,
    ]);
    let stderr = normalize(&String::from_utf8_lossy(&out.stderr), &s);
    let stdout = normalize(&String::from_utf8_lossy(&out.stdout), &s);
    assert_eq!(out.status.code(), Some(125));
    assert_eq!(stderr, GOLDEN_NO_TTY_STDERR.trim_matches('\n'));
    assert_eq!(stdout, GOLDEN_NO_TTY_STDOUT);
}

const GOLDEN_GOOD_PASSWORD: &str = r"
goway: info: newbox at 127.0.0.1:2222 answers but refuses key login; setting it up
goway: note: created goway's own key <config>/id_ed25519
goway: note: To let this laptop log in to newbox without a password from now on, goway puts a key on newbox once.
goway: note: (If tester has no password, or you are not sure, answer n: goway shows three lines to paste on newbox instead.)
goway: Do you know the password of tester on newbox? [Y/n] y
goway: note: ssh now asks for tester's password on newbox (typing is hidden; goway never sees or stores it).
goway: done: key login to newbox works; undo with `goway ssh setup newbox --undo`
";
const GOLDEN_BAD_PASSWORD: &str = r#"
goway: info: newbox at 127.0.0.1:2222 answers but refuses key login; setting it up
goway: note: created goway's own key <config>/id_ed25519
goway: note: To let this laptop log in to newbox without a password from now on, goway puts a key on newbox once.
goway: note: (If tester has no password, or you are not sure, answer n: goway shows three lines to paste on newbox instead.)
goway: Do you know the password of tester on newbox? [Y/n] y
goway: note: ssh now asks for tester's password on newbox (typing is hidden; goway never sees or stores it).
goway: warning: That password did not work on newbox.
goway: note: Most likely tester has no password set (common with automatic login) or newbox only allows key logins; other causes are in docs/troubleshooting.md, "goway add says Permission denied".
goway: info: Let's do it the other way:
goway: info: On newbox, open a terminal and paste these three lines:

mkdir -p ~/.ssh && chmod 700 ~/.ssh
echo '<restricted key line>' >> ~/.ssh/authorized_keys
chmod 600 ~/.ssh/authorized_keys

goway: Then press Enter here (Ctrl-C to stop). 
goway: done: Key works. newbox is ready.
goway: done: key login to newbox works; undo with `goway ssh setup newbox --undo`
"#;
const GOLDEN_NO_PASSWORD: &str = r"
goway: info: newbox at 127.0.0.1:2222 answers but refuses key login; setting it up
goway: note: created goway's own key <config>/id_ed25519
goway: note: To let this laptop log in to newbox without a password from now on, goway puts a key on newbox once.
goway: note: (If tester has no password, or you are not sure, answer n: goway shows three lines to paste on newbox instead.)
goway: Do you know the password of tester on newbox? [Y/n] n
goway: note: No password: goway will show you what to paste on newbox instead.
goway: info: On newbox, open a terminal and paste these three lines:

mkdir -p ~/.ssh && chmod 700 ~/.ssh
echo '<restricted key line>' >> ~/.ssh/authorized_keys
chmod 600 ~/.ssh/authorized_keys

goway: Then press Enter here (Ctrl-C to stop). 
goway: done: Key works. newbox is ready.
goway: done: key login to newbox works; undo with `goway ssh setup newbox --undo`
";
const GOLDEN_NO_TTY_STDERR: &str = r#"
goway: info: newbox at 127.0.0.1:2222 answers but refuses key login; setting it up
goway: note: created goway's own key <config>/id_ed25519
goway: note: ssh now asks for tester's password on newbox (typing is hidden; goway never sees or stores it).
goway: warning: That password did not work on newbox.
goway: note: Most likely tester has no password set (common with automatic login) or newbox only allows key logins; other causes are in docs/troubleshooting.md, "goway add says Permission denied".
goway: info: Let's do it the other way:
goway: info: On newbox, open a terminal and paste these three lines:
goway: next: run the lines above on newbox, then rerun the same `goway add newbox` command
goway: error: cannot add host `newbox`: goway's key is not installed there yet, and there is no terminal to wait on
"#;
const GOLDEN_NO_TTY_STDOUT: &str = "\nmkdir -p ~/.ssh && chmod 700 ~/.ssh\necho '<restricted key line>' >> ~/.ssh/authorized_keys\nchmod 600 ~/.ssh/authorized_keys\n";
