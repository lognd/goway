//! Pinned tools installed by `doctor --fix` are journaled resources: undo removes exactly the
//! tree and links the install added and restores a link it replaced.
#![cfg(target_os = "linux")]

mod common;

use goway::remotesys::RemoteSystem;
use goway::ssh::{Settings, Target};
use std::path::{Path, PathBuf};
use std::process::Command;

use goway_journal::{Change, Journal, Outcome, ResourceKind, revert};

const SPEC: &str = "mkdir -p \"$HOME/.local/opt/goway-tool/bin\" \"$HOME/.local/bin\" && echo x > \"$HOME/.local/opt/goway-tool/bin/tool\" && ln -sf \"$HOME/.local/opt/goway-tool/bin/tool\" \"$HOME/.local/bin/tool\"";

fn change() -> Change {
    Change::EnsureResource {
        kind: ResourceKind::PinnedTool,
        name: "tool:tool".to_owned(),
        spec: SPEC.to_owned(),
    }
}

/// The remote system the inner steps use: a fake ssh that runs the command on this machine.
fn system(known_hosts: PathBuf) -> RemoteSystem {
    RemoteSystem {
        target: Target {
            name: "helios".to_owned(),
            address: "127.0.0.1".to_owned(),
            port: 22,
            user: None,
            identity: None,
        },
        settings: Settings {
            known_hosts,
            control_dir: None,
            connect_timeout_secs: 5,
        },
        password: false,
        prompt: None,
    }
}

/// The step the outer test asked for, run in a child process whose PATH has the fake ssh and
/// whose HOME is the fake remote's home (`apply` writes the journal, `revert` replays it).
// frob:tests crates/goway/src/remotesys.rs::RemoteSystem
#[test]
#[ignore = "run by the tests below in a child process with a fake ssh"]
fn inner_step() {
    let journal_path = PathBuf::from(std::env::var("PINNED_JOURNAL").unwrap());
    let mut sys = system(journal_path.with_extension("kh"));
    match std::env::var("PINNED_STEP").unwrap().as_str() {
        "apply" => {
            let journal = goway_journal::apply(&[change()], &mut sys).unwrap();
            journal.save(&journal_path).unwrap();
        }
        "apply_refused" => assert!(goway_journal::apply(&[change()], &mut sys).is_err()),
        _ => {
            let mut journal = Journal::load(&journal_path).unwrap();
            let report = revert(&mut journal, &mut sys).unwrap();
            assert_eq!(report.outcomes[0].1, Outcome::Restored);
        }
    }
}

fn step(w: &common::World, home: &Path, step: &str) {
    let path = format!(
        "{}:{}",
        w.bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let out = Command::new(std::env::current_exe().unwrap())
        .args(["--ignored", "--exact", "inner_step", "--nocapture"])
        .env("PATH", path)
        .env("HOME", home)
        .env("PINNED_STEP", step)
        .env("PINNED_JOURNAL", w.root.join("journal.json"))
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{step}: {}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

fn world_with_home() -> (common::World, PathBuf) {
    let w = common::world();
    let home = w.root.join("home");
    std::fs::create_dir_all(home.join(".local/bin")).unwrap();
    (w, home)
}

// frob:tests crates/goway/src/remotesys.rs::RemoteSystem
#[test]
fn undo_removes_the_tree_and_links_and_restores_a_replaced_link() {
    let (w, home) = world_with_home();
    let bin = home.join(".local/bin");
    std::os::unix::fs::symlink("/usr/bin/true", bin.join("tool")).unwrap();
    step(&w, &home, "apply");
    let link = std::fs::read_link(bin.join("tool")).unwrap();
    assert!(
        link.starts_with(home.join(".local/opt/goway-tool")),
        "{link:?}"
    );
    step(&w, &home, "revert");
    assert!(!home.join(".local/opt").exists(), "the tree is gone");
    assert_eq!(
        std::fs::read_link(bin.join("tool")).unwrap(),
        Path::new("/usr/bin/true"),
        "the replaced link is back"
    );
}

#[test]
fn undo_leaves_nothing_behind_when_no_link_was_replaced() {
    let (w, home) = world_with_home();
    step(&w, &home, "apply");
    assert!(home.join(".local/opt/goway-tool/bin/tool").is_file());
    step(&w, &home, "revert");
    assert!(!home.join(".local/opt").exists());
    assert!(!home.join(".local/bin/tool").exists());
}

#[test]
fn a_regular_file_in_the_way_is_never_replaced() {
    let (w, home) = world_with_home();
    std::fs::write(home.join(".local/bin/tool"), "mine").unwrap();
    step(&w, &home, "apply_refused");
    assert_eq!(
        std::fs::read_to_string(home.join(".local/bin/tool")).unwrap(),
        "mine"
    );
    assert!(!home.join(".local/opt/goway-tool").exists());
}
