//! Clock safety: a step of the helper's wall clock never exposes a starting
//! run to gc, and files dated in the helper's future neither warn nor make
//! the next run rebuild.
#![cfg(unix)]

mod common;

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

fn uptime() -> u64 {
    std::fs::read_to_string("/proc/uptime")
        .ok()
        .and_then(|t| t.split('.').next().and_then(|s| s.parse().ok()))
        .unwrap_or(0)
}

/// gc at a wall-clock time `years` ahead of the real one, applying removals.
fn gc_in_the_future(home: &Path, years: u64) -> Output {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + years * 365 * 86_400;
    remote(
        home,
        "gc",
        &[
            ".cache/goway",
            &now.to_string(),
            "0",
            "0",
            "0",
            "apply",
            "",
            "0",
        ],
    )
}

fn work_dir(root: &Path, name: &str, born: Option<u64>) -> std::path::PathBuf {
    let dir = root.join("work").join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("meta.json"),
        r#"{"kind":"work","repo":"r","repo_id":"i"}"#,
    )
    .unwrap();
    if let Some(b) = born {
        std::fs::write(dir.join("born"), format!("{b}\n")).unwrap();
    }
    dir
}

// frob:tests crates/goway/src/remote.rs::SCRIPT
#[test]
fn a_clock_jump_never_lets_gc_remove_a_run_that_is_starting() {
    if uptime() == 0 {
        return; // no /proc/uptime on this host: the marker is not written either
    }
    let home = tempfile::tempdir().unwrap();
    let root = home.path().join(".cache/goway");
    assert!(
        remote(home.path(), "manifest", &[".cache/goway", "abc"])
            .status
            .success()
    );
    let starting = work_dir(&root, "starting", Some(uptime()));
    // Born at boot (uptime 0); only old by the monotonic clock once the host
    // has been up longer than the 120 s grace (plus a margin), which a freshly
    // booted CI runner or helper may not have.
    let long_ago = work_dir(&root, "long-ago", Some(0));
    let long_ago_is_old = uptime() >= 150;
    let unmarked = work_dir(&root, "unmarked", None);
    // The helper's wall clock jumps forward by ten years.
    let out = gc_in_the_future(home.path(), 10);
    assert!(out.status.success(), "{out:?}");
    assert!(
        starting.exists(),
        "a starting run was removed after a clock jump"
    );
    if long_ago_is_old {
        assert!(!long_ago.exists(), "an old dir by the monotonic clock goes");
    }
    assert!(
        !unmarked.exists(),
        "a dir without the marker follows the age rule"
    );
}

// frob:tests crates/goway/src/remote.rs::SCRIPT
#[test]
fn a_dir_whose_creator_process_is_alive_is_never_removed() {
    let home = tempfile::tempdir().unwrap();
    let root = home.path().join(".cache/goway");
    assert!(
        remote(home.path(), "manifest", &[".cache/goway", "abc"])
            .status
            .success()
    );
    let dir = work_dir(&root, "mine", Some(0));
    // This test process is the creator; it is alive.
    std::fs::write(dir.join("creator"), format!("{}\n", std::process::id())).unwrap();
    assert!(gc_in_the_future(home.path(), 10).status.success());
    assert!(dir.exists(), "a dir with a live creator was removed");
    // A dead creator (a pid that is not running) no longer protects it.
    std::fs::write(dir.join("creator"), "999999999\n").unwrap();
    assert!(gc_in_the_future(home.path(), 10).status.success());
    assert!(!dir.exists());
}

/// Dates `file` `ahead` seconds after now.
fn date_ahead(file: &Path, ahead: u64) {
    let when = std::time::SystemTime::now() + std::time::Duration::from_secs(ahead);
    std::fs::File::options()
        .write(true)
        .open(file)
        .unwrap()
        .set_modified(when)
        .unwrap();
}

// frob:tests crates/goway/src/remote.rs::SCRIPT
#[test]
fn files_dated_an_hour_ahead_neither_warn_nor_rebuild_on_the_second_run() {
    let w = common::world();
    std::fs::write(w.repo.join(".gitignore"), "out\n").unwrap();
    let src = w.repo.join("src.txt");
    std::fs::write(&src, "from the laptop\n").unwrap();
    date_ahead(&src, 3600);
    let build =
        "if [ ! -e out ] || [ src.txt -nt out ]; then cp src.txt out; echo rebuilt; fi; cat out";
    let first = w.run(&["run", "--", "sh", "-c", build]);
    assert_eq!(
        String::from_utf8_lossy(&first.stdout),
        "rebuilt\nfrom the laptop\n"
    );
    let second = w.run(&["run", "--", "sh", "-c", build]);
    assert_eq!(
        String::from_utf8_lossy(&second.stdout),
        "from the laptop\n",
        "the second run rebuilt a file dated in the future"
    );
    for out in [&first, &second] {
        let err = String::from_utf8_lossy(&out.stderr).to_lowercase();
        assert!(!err.contains("future") && !err.contains("skew"), "{err}");
    }
}
