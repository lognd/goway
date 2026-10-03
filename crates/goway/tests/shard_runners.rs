//! `goway run --shard N` with real test frameworks (pytest, go test,
//! `CTest`) through the fake-ssh world: together the shards must run every
//! test exactly once. A framework that is not installed skips its test.
#![cfg(unix)]
#![allow(clippy::format_collect)] // building small fixture files

mod common;

use std::collections::BTreeMap;

fn have(tool: &str) -> bool {
    std::process::Command::new(tool)
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

/// Two configured hosts that are both the fake local one.
fn two_hosts() -> common::World {
    let w = common::world();
    let mut config = std::fs::read_to_string(w.config.join("config.toml")).unwrap();
    config = config.replace("name = \"local\"", "name = \"alpha\"");
    config.push_str("\n[[host]]\nname = \"beta\"\naddress = \"127.0.0.1\"\n");
    std::fs::write(w.config.join("config.toml"), config).unwrap();
    w
}

fn write(w: &common::World, path: &str, text: &str) {
    let p = w.repo.join(path);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

/// Run sharded in two parts; return how often each line containing `marker`
/// (after the host prefix is stripped) appeared, plus the exit code.
fn counts(
    w: &common::World,
    args: &[&str],
    marker: &str,
) -> (BTreeMap<String, usize>, Option<i32>) {
    let mut full = vec!["run", "--shard", "2", "--"];
    full.extend_from_slice(args);
    let out = w.run(&full);
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let mut seen = BTreeMap::new();
    for line in stdout.lines().filter(|l| l.contains(marker)) {
        let line = line.split_once("] ").map_or(line, |(_, rest)| rest);
        *seen.entry(line.trim().to_owned()).or_insert(0) += 1;
    }
    assert!(
        out.status.success() || !seen.is_empty(),
        "{}\n{stdout}",
        String::from_utf8_lossy(&out.stderr)
    );
    (seen, out.status.code())
}

// frob:tests crates/goway/src/runners.rs::plan
// frob:tests crates/goway/src/shard.rs::run_sharded
#[test]
fn pytest_shards_run_every_test_exactly_once() {
    if !have("pytest") {
        return;
    }
    let w = two_hosts();
    for (file, n) in [("a", 3), ("b", 2), ("sub/c", 4)] {
        let body: String = (0..n)
            .map(|i| format!("def test_{i}():\n    assert True\n\n"))
            .collect();
        write(
            &w,
            &format!("tests/test_{}.py", file.replace('/', "_")),
            &body,
        );
    }
    let (seen, code) = counts(&w, &["pytest", "-v", "-p", "no:cacheprovider"], "PASSED");
    assert_eq!(code, Some(0));
    assert_eq!(seen.len(), 9, "{seen:?}");
    assert!(seen.values().all(|&n| n == 1), "{seen:?}");
}

// frob:tests crates/goway/src/runners.rs::plan
#[test]
fn go_test_shards_run_every_test_exactly_once() {
    if !have("go") {
        return;
    }
    let w = two_hosts();
    write(&w, "go.mod", "module example.com/m\n\ngo 1.20\n");
    for (dir, names) in [
        ("a", ["TestA1", "TestA2"]),
        ("b", ["TestB1", "TestB2"]),
        ("c", ["TestC1", "TestC2"]),
    ] {
        let body: String = names
            .iter()
            .map(|n| format!("func {n}(t *testing.T) {{}}\n\n"))
            .collect();
        write(
            &w,
            &format!("{dir}/{dir}_test.go"),
            &format!("package {dir}\n\nimport \"testing\"\n\n{body}"),
        );
    }
    let (seen, code) = counts(&w, &["go", "test", "-v", "-count=1", "./..."], "--- PASS");
    assert_eq!(code, Some(0));
    assert_eq!(seen.len(), 6, "{seen:?}");
    assert!(seen.values().all(|&n| n == 1), "{seen:?}");
}

// frob:tests crates/goway/src/runners.rs::plan
#[test]
fn ctest_shards_run_every_test_exactly_once() {
    if !have("ctest") {
        return;
    }
    let w = two_hosts();
    let tests: String = (1..=7)
        .map(|i| format!("add_test(t{i} \"echo\" \"t{i}\")\n"))
        .collect();
    write(&w, "CTestTestfile.cmake", &tests);
    let (seen, code) = counts(&w, &["ctest"], "Test #");
    assert_eq!(code, Some(0));
    // Test numbers are per shard (the stride keeps the original numbers).
    let names: Vec<&str> = seen
        .keys()
        .filter_map(|l| l.split(": ").nth(1))
        .filter_map(|r| r.split(' ').next())
        .collect();
    let mut sorted = names.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(names.len(), 7, "{seen:?}");
    assert_eq!(sorted.len(), 7, "{seen:?}");
}

// frob:tests crates/goway/src/runners.rs::plan
#[test]
fn a_command_that_already_shards_is_refused_before_any_host_is_used() {
    let w = two_hosts();
    let out = w.run(&["run", "--shard", "2", "--", "npx", "vitest", "--shard=1/2"]);
    assert_eq!(out.status.code(), Some(125));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("already splits its tests"), "{err}");
    assert!(w.work_dirs().is_empty());
}
