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
        "{}\n--- stdout ---\n{}",
        String::from_utf8_lossy(&out.stderr),
        String::from_utf8_lossy(&out.stdout)
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
    let probe = "cat hello.txt other.txt";
    assert_eq!(sh(&w, probe), "hello\nother\n");
    assert_eq!(stats(&w, 0), "written=2 removed=0");
    assert_eq!(sh(&w, probe), "hello\nother\n");
    assert_eq!(
        stats(&w, 0),
        "written=0 removed=0",
        "no file is copied again"
    );

    std::fs::write(w.repo.join("hello.txt"), "changed\n").unwrap();
    assert_eq!(sh(&w, probe), "changed\nother\n");
    assert_eq!(
        stats(&w, 0),
        "written=1 removed=0",
        "only the edit is written"
    );
}

/// A same-size edit within the same whole second as the previous sync (the
/// resolution of the mtimes goway compares) still reaches the slot. The
/// stamp is dated ahead so it stays inside the racy window (at or after the
/// second before a sync starts) however long a loaded host takes per run;
/// a stamp fixed at test start ages out of it and would then be, by design,
/// indistinguishable from an unchanged file.
#[test]
fn same_size_edit_within_the_same_second_reaches_the_slot() {
    let w = world();
    let file = w.repo.join("hello.txt");
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let stamp = std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs + 3600);
    for content in ["aaaa\n", "bbbb\n", "cccc\n", "dddd\n"] {
        std::fs::write(&file, content).unwrap();
        std::fs::File::options()
            .write(true)
            .open(&file)
            .unwrap()
            .set_modified(stamp)
            .unwrap();
        assert_eq!(sh(&w, "cat hello.txt"), content);
    }
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
    let busy = w.hold(&[], "pwd");
    busy.wait_started();
    std::fs::write(w.repo.join("hello.txt"), "changed\n").unwrap();
    let second = sh(&w, "pwd; cat hello.txt");
    assert!(second.contains("/tree-1\n"), "{second}");
    assert!(second.contains("changed"), "{second}");
    assert!(busy.finish().status.success());
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
    let busy = w.hold(&[], "true");
    busy.wait_started();
    assert!(sh(&w, "pwd").contains("/tree-1\n"));
    assert!(busy.finish().status.success());
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

/// Set `file`'s content and mtime (`ago` seconds before now).
fn write_dated(file: &Path, content: &str, ago: u64) {
    std::fs::write(file, content).unwrap();
    let when = std::time::SystemTime::now() - std::time::Duration::from_secs(ago);
    std::fs::File::options()
        .write(true)
        .open(file)
        .unwrap()
        .set_modified(when)
        .unwrap();
}

/// Apple's bash 3.2 (`-nt`) and make 3.81 compare whole seconds only: let
/// the next write fall in a later second than what was just built.
fn next_second() {
    std::thread::sleep(std::time::Duration::from_millis(1100));
}

/// Build A in a slot, then run a worktree state B whose changed source has
/// an older mtime than A's output: the build must still see B's source.
const STAMP_BUILD: &str =
    "if [ ! -e out ] || [ src.txt -nt out ]; then cp src.txt out; echo rebuilt; fi; cat out";

#[test]
fn changed_files_are_stamped_newer_than_the_slots_outputs() {
    let w = world();
    let src = w.repo.join("src.txt");
    std::fs::write(w.repo.join(".gitignore"), "out\n").unwrap();
    write_dated(&src, "from A\n", 0);
    assert_eq!(sh(&w, STAMP_BUILD), "rebuilt\nfrom A\n");
    // An older branch: different content, a much older mtime.
    next_second();
    write_dated(&src, "from B\n", 3600);
    assert_eq!(
        sh(&w, STAMP_BUILD),
        "rebuilt\nfrom B\n",
        "the build saw B's source"
    );
    // Nothing changed: the warm build stays warm (no rebuild, no rewrite).
    assert_eq!(sh(&w, STAMP_BUILD), "from B\n");
    assert_eq!(stats(&w, 0), "written=0 removed=0");
}

#[test]
fn make_rebuilds_when_an_older_branch_reuses_the_slot() {
    if Command::new("make").arg("--version").output().is_err() {
        return;
    }
    let w = world();
    std::fs::write(w.repo.join("Makefile"), "out: src.txt\n\tcp src.txt out\n").unwrap();
    std::fs::write(w.repo.join(".gitignore"), "out\n").unwrap();
    write_dated(&w.repo.join("src.txt"), "from A\n", 0);
    assert_eq!(sh(&w, "make -s && cat out"), "from A\n");
    next_second();
    write_dated(&w.repo.join("src.txt"), "from B\n", 3600);
    assert_eq!(sh(&w, "make -s && cat out"), "from B\n");
}

#[test]
fn cargo_rebuilds_when_an_older_branch_reuses_the_slot() {
    if Command::new("cargo").arg("--version").output().is_err() {
        return;
    }
    let w = world();
    std::fs::write(
        w.repo.join("Cargo.toml"),
        "[package]\nname = \"fx\"\nversion = \"0.0.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    std::fs::create_dir(w.repo.join("src")).unwrap();
    let main = w.repo.join("src/main.rs");
    let run = "cargo run -q --offline 2>&1";
    write_dated(&main, "fn main() { println!(\"from A\"); }\n", 0);
    assert_eq!(sh(&w, run), "from A\n");
    write_dated(&main, "fn main() { println!(\"from B\"); }\n", 3600);
    assert_eq!(
        sh(&w, run),
        "from B\n",
        "cargo saw B's older source as changed"
    );
}

#[test]
fn written_files_are_stamped_after_outputs_dated_in_the_future() {
    let w = world();
    std::fs::write(w.repo.join(".gitignore"), "out\n").unwrap();
    write_dated(&w.repo.join("src.txt"), "from A\n", 0);
    sh(&w, "cp src.txt out; touch -d '+2 days' out");
    write_dated(&w.repo.join("src.txt"), "from B\n", 3600);
    assert_eq!(sh(&w, STAMP_BUILD), "rebuilt\nfrom B\n");
}
