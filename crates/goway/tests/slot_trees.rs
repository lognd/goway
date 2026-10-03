//! Persistent slot trees: a slot's tree is updated in place from the run's
//! snapshot, dependency dirs stay warm, and gc never races a lock.
#![cfg(unix)]

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;

use common::{World, world};

fn ok(out: &std::process::Output) -> String {
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn sh(w: &World, script: &str) -> String {
    ok(&w.run(&["run", "--", "sh", "-c", script]))
}

fn cache_dir(w: &World) -> PathBuf {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(w.remote.join("cache"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    assert_eq!(dirs.len(), 1, "{dirs:?}");
    dirs.pop().unwrap()
}

fn stats(w: &World, slot: u32) -> String {
    std::fs::read_to_string(cache_dir(w).join(format!("tree-{slot}.stats")))
        .unwrap()
        .trim()
        .to_owned()
}

fn set_config(w: &World, extra: &str) {
    let path = w.config.join("config.toml");
    let text = std::fs::read_to_string(&path).unwrap();
    std::fs::write(
        path,
        text.replace(
            "target_slots = 2\n",
            &format!("target_slots = 2\n{extra}\n"),
        ),
    )
    .unwrap();
}

#[test]
fn second_run_updates_in_place_and_writes_only_changes() {
    let w = world();
    std::fs::write(w.repo.join("other.txt"), "other\n").unwrap();
    let probe = "stat -c '%i %Y' hello.txt other.txt";
    let first = sh(&w, probe);
    assert_eq!(stats(&w, 0), "written=2 removed=0");
    let second = sh(&w, probe);
    assert_eq!(stats(&w, 0), "written=0 removed=0");
    assert_eq!(first, second, "untouched files keep inode and mtime");

    std::fs::write(w.repo.join("hello.txt"), "changed\n").unwrap();
    let third = sh(&w, probe);
    assert_eq!(stats(&w, 0), "written=1 removed=0");
    let (a, b) = (
        first.lines().collect::<Vec<_>>(),
        third.lines().collect::<Vec<_>>(),
    );
    assert_ne!(a[0], b[0], "the changed file was rewritten");
    assert_eq!(a[1], b[1], "the other file was not");

    // The seed's mtime is preserved, not the time of the copy.
    let local = std::fs::metadata(w.repo.join("other.txt")).unwrap();
    let secs = local
        .modified()
        .unwrap()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    assert_eq!(b[1].split(' ').nth(1).unwrap(), secs.to_string());
}

#[test]
fn deleted_files_and_leftovers_go_but_dependency_dirs_stay() {
    let w = world();
    for f in ["package.json", "pyproject.toml", "CMakeLists.txt"] {
        std::fs::write(w.repo.join(f), "{}\n").unwrap();
    }
    sh(
        &w,
        "mkdir -p node_modules/x .venv build sub/__pycache__ leftover_dir; \
         touch node_modules/x/i.js .venv/p build/o sub/__pycache__/c generated.txt leftover_dir/f",
    );
    std::fs::remove_file(w.repo.join("hello.txt")).unwrap();
    let out = sh(
        &w,
        "ls -A . sub; ls node_modules/x .venv build sub/__pycache__",
    );
    for kept in ["i.js", "p", "o", "c", "node_modules", ".venv", "build"] {
        assert!(out.contains(kept), "{kept} missing:\n{out}");
    }
    for gone in ["hello.txt", "generated.txt", "leftover_dir"] {
        assert!(!out.contains(gone), "{gone} survived:\n{out}");
    }
}

#[test]
fn gitignored_paths_stay_unless_switched_off_and_config_keep_list_stays() {
    let w = world();
    std::fs::write(w.repo.join(".gitignore"), "dist/\n").unwrap();
    set_config(&w, "keep = [\"scratch\", \"out/cache.bin\"]");
    let make = "mkdir -p dist out scratch; touch dist/a out/cache.bin out/other scratch/s";
    sh(&w, make);
    let out = sh(&w, "ls dist out scratch");
    assert!(
        out.contains('a') && out.contains("cache.bin") && out.contains('s'),
        "{out}"
    );
    assert!(!out.contains("other"), "{out}");

    set_config(&w, "keep_ignored = false");
    let out = sh(&w, "ls -A dist 2>&1; ls scratch");
    assert!(
        out.contains("No such file"),
        "dist/ is no longer kept:\n{out}"
    );
}

#[test]
fn overlapping_runs_use_different_slots_and_the_seed_is_not_shared() {
    let w = world();
    let mut busy = w
        .goway(&["run", "--", "sh", "-c", "pwd; sleep 3; cat hello.txt"])
        .spawn()
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(1500));
    std::fs::write(w.repo.join("hello.txt"), "changed\n").unwrap();
    let second = sh(&w, "pwd; cat hello.txt");
    assert!(second.contains("/tree-1\n"), "{second}");
    assert!(second.contains("changed"), "{second}");
    assert!(busy.wait().unwrap().success());
    // Slot trees never share inodes with the seed a later sync mutates.
    let inode = |p: &Path| {
        use std::os::unix::fs::MetadataExt as _;
        std::fs::metadata(p).unwrap().ino()
    };
    let seed_file = walk(&w.remote.join("seed"), "hello.txt");
    assert_ne!(
        inode(&seed_file),
        inode(&cache_dir(&w).join("tree-1/hello.txt"))
    );
}

fn walk(dir: &Path, name: &str) -> PathBuf {
    find_file(dir, name).unwrap_or_else(|| panic!("{name} not found under {}", dir.display()))
}

fn find_file(dir: &Path, name: &str) -> Option<PathBuf> {
    for e in std::fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        if p.is_dir() {
            if let Some(f) = find_file(&p, name) {
                return Some(f);
            }
        } else if p.file_name().unwrap() == name {
            return Some(p);
        }
    }
    None
}

#[test]
fn a_run_prefers_the_slot_its_worktree_used_last() {
    let w = world();
    let mut busy = w
        .goway(&["run", "--", "sh", "-c", "sleep 2"])
        .spawn()
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(1200));
    assert!(sh(&w, "pwd").contains("/tree-1\n"));
    assert!(busy.wait().unwrap().success());
    // Slot 0 is free again, but this worktree last used slot 1.
    assert!(sh(&w, "pwd").contains("/tree-1\n"));
}

#[test]
fn keep_copies_the_tree_out_and_the_slot_stays_usable() {
    let w = world();
    ok(&w.run(&["run", "--keep", "--", "sh", "-c", "touch made.txt"]));
    let dir = w.remote.join("work").join(&w.work_dirs()[0]);
    assert!(dir.join("tree/hello.txt").is_file());
    assert!(dir.join("tree/made.txt").is_file());
    assert!(
        !sh(&w, "ls").contains("made.txt"),
        "leftover removed from the slot"
    );
}

// frob:tests crates/goway/src/gc.rs::gc
#[test]
fn gc_removes_an_idle_slot_tree_with_its_cache() {
    let w = world();
    sh(&w, "true");
    let cache = cache_dir(&w);
    assert!(cache.join("tree-0").is_dir());
    let ok_touch = Command::new("touch")
        .args(["-d", "8 days ago"])
        .arg(cache.join("meta.json"))
        .status()
        .unwrap();
    assert!(ok_touch.success());
    ok(&w.run(&["gc"]));
    assert!(!cache.exists(), "the idle cache and its slot tree are gone");
}

/// gc removing a seed between a sync's mkdir and its lock open (or after it
/// opened the lock) must not fail the sync: the lock is retaken.
#[test]
fn locking_a_seed_survives_gc_removing_it_mid_acquire() {
    let w = world();
    let script =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/remote.sh"))
            .unwrap();
    let root = w.root.join("hook-root");
    let seed = root.join("seed/repo/wt");
    let out = Command::new("bash")
        .args(["-c", &script, "goway", "manifest"])
        .arg(&root)
        .arg("repo/wt")
        .env("GOWAY_TEST_HOOK", format!("rm -rf '{}'", seed.display()))
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        seed.join("lock").exists(),
        "the lock was retaken in a fresh dir"
    );
}
