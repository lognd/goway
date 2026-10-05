//! The disk budget: least-recently-used eviction of unlocked entries.
#![cfg(unix)]

use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const SCRIPT: &str = include_str!("../src/remote.sh");
const MIB: u64 = 1 << 20;
const TEN_YEARS: &str = "315360000";

fn remote(home: &Path, verb: &str, args: &[&str]) -> Output {
    Command::new("bash")
        .args(["-c", SCRIPT, "goway", verb])
        .args(args)
        .env("HOME", home)
        .output()
        .unwrap()
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

/// Set a file's mtime to `age` seconds ago.
fn age(path: &Path, secs: u64) {
    let when = std::time::SystemTime::now() - std::time::Duration::from_secs(secs);
    std::fs::File::options()
        .write(true)
        .open(path)
        .or_else(|_| std::fs::File::open(path))
        .unwrap()
        .set_modified(when)
        .unwrap();
}

/// A marked root with one repository cache per `(name, [slot ages])`; every
/// slot holds a 1 MiB tree.
fn fake_root(home: &Path, repos: &[(&str, &[u64])]) -> PathBuf {
    let root = home.join(".cache/goway");
    assert!(
        remote(home, "manifest", &[".cache/goway", "abc"])
            .status
            .success()
    );
    for (name, slots) in repos {
        let cache = root.join("cache").join(name);
        std::fs::create_dir_all(&cache).unwrap();
        std::fs::write(
            cache.join("meta.json"),
            format!(r#"{{"kind":"cache","repo":"{name}","repo_id":"id-{name}"}}"#),
        )
        .unwrap();
        for (k, secs) in slots.iter().enumerate() {
            std::fs::create_dir_all(cache.join(format!("tree-{k}"))).unwrap();
            let f = std::fs::File::create(cache.join(format!("tree-{k}/big"))).unwrap();
            f.set_len(MIB).unwrap();
            let lock = cache.join(format!("target-{k}.lock"));
            std::fs::write(&lock, "").unwrap();
            age(&lock, *secs);
        }
        age(&cache.join("meta.json"), *slots.iter().min().unwrap());
    }
    root
}

fn gc(home: &Path, mode: &str, max_disk: u64) -> String {
    let out = remote(
        home,
        "gc",
        &[
            ".cache/goway",
            &now().to_string(),
            TEN_YEARS,
            TEN_YEARS,
            TEN_YEARS,
            mode,
            "",
            "",
            &max_disk.to_string(),
            "0",
        ],
    );
    assert!(out.status.success(), "{out:?}");
    String::from_utf8(out.stdout).unwrap()
}

fn lines(out: &str, action: &str) -> Vec<String> {
    out.lines()
        .filter(|l| l.starts_with(&format!("{action}\t")))
        .map(str::to_owned)
        .collect()
}

#[test]
fn eviction_removes_the_least_recently_used_slot_first_and_reports_it() {
    let home = tempfile::tempdir().unwrap();
    // alpha's slot 1 is the oldest, then alpha 0, then beta 0.
    let root = fake_root(home.path(), &[("alpha", &[3000, 9000]), ("beta", &[1000])]);
    // Three MiB used; room for about two and a half: exactly one slot goes.
    let out = gc(home.path(), "apply", 5 * MIB / 2);
    let evicted = lines(&out, "evict");
    assert_eq!(evicted.len(), 1, "{out}");
    assert!(evicted[0].contains("\tslot\t"), "{out}");
    assert!(evicted[0].ends_with("cache/alpha/tree-1"), "{out}");
    let f: Vec<&str> = evicted[0].split('\t').collect();
    assert!(
        f[3].parse::<u64>().unwrap() >= MIB,
        "bytes freed reported: {out}"
    );
    assert!(!root.join("cache/alpha/tree-1").exists());
    assert!(root.join("cache/alpha/tree-0").exists());
    assert!(root.join("cache/beta/tree-0").exists());
}

#[test]
fn eviction_keeps_a_locked_slot_and_takes_the_next_oldest() {
    let home = tempfile::tempdir().unwrap();
    let root = fake_root(home.path(), &[("alpha", &[9000, 5000]), ("beta", &[1000])]);
    let lock = root.join("cache/alpha/target-0.lock");
    // A run holds the oldest slot for the whole eviction.
    // Its own process group, so the whole group (flock and its sleep) can be killed.
    let mut holder = Command::new("flock")
        .process_group(0)
        .arg("-x")
        .arg(&lock)
        .args(["sleep", "30"])
        .spawn()
        .unwrap();
    let start = std::time::Instant::now();
    while Command::new("flock")
        .args(["-n"])
        .arg(&lock)
        .arg("true")
        .status()
        .unwrap()
        .success()
    {
        assert!(start.elapsed().as_secs() < 20, "flock never took the lock");
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let out = gc(home.path(), "apply", 5 * MIB / 2);
    let _ = Command::new("kill")
        .args(["-KILL", "--"])
        .arg(format!("-{}", holder.id()))
        .status();
    holder.wait().unwrap();
    let busy = lines(&out, "busy");
    assert!(
        busy.iter()
            .any(|l| l.contains("\tslot\t") && l.ends_with("alpha/tree-0")),
        "{out}"
    );
    let evicted = lines(&out, "evict");
    assert_eq!(evicted.len(), 1, "{out}");
    assert!(evicted[0].ends_with("cache/alpha/tree-1"), "{out}");
    assert!(root.join("cache/alpha/tree-0").exists(), "locked slot kept");
    assert!(root.join("cache/beta/tree-0").exists());
}

#[test]
fn dry_run_lists_what_eviction_would_remove_and_removes_nothing() {
    let home = tempfile::tempdir().unwrap();
    let root = fake_root(home.path(), &[("alpha", &[3000, 9000]), ("beta", &[1000])]);
    let out = gc(home.path(), "dry", 5 * MIB / 2);
    let evicted = lines(&out, "evict");
    assert_eq!(evicted.len(), 1, "{out}");
    assert!(evicted[0].ends_with("cache/alpha/tree-1"), "{out}");
    assert!(root.join("cache/alpha/tree-1/big").exists());
    // Within budget: nothing to evict.
    assert!(lines(&gc(home.path(), "dry", 100 * MIB), "evict").is_empty());
}

#[test]
fn run_leaves_a_summary_of_what_the_budget_freed_and_caps_compiler_caches() {
    let home = tempfile::tempdir().unwrap();
    let root = fake_root(home.path(), &[("alpha", &[9000])]);
    let out = remote(
        home.path(),
        "gc",
        &[
            ".cache/goway",
            &now().to_string(),
            TEN_YEARS,
            TEN_YEARS,
            TEN_YEARS,
            "apply",
            "",
            "",
            &(MIB / 2).to_string(),
            "0",
            "log",
        ],
    );
    assert!(out.status.success(), "{out:?}");
    let log = std::fs::read_to_string(root.join("evicted.log")).unwrap();
    assert!(log.contains("evicted 1 entries, freed 1.0 MiB"), "{log}");
    assert!(SCRIPT.contains("SCCACHE_CACHE_SIZE=\"${SCCACHE_CACHE_SIZE:-"));
    assert!(SCRIPT.contains("CCACHE_MAXSIZE=\"${CCACHE_MAXSIZE:-"));
}
