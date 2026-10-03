//! `goway run --shard N` detecting `GoogleTest` and Catch2 v3 binaries by
//! the flag strings embedded in them, through the fake-ssh world.
//!
//! The real frameworks are not installed here (no gtest or Catch2 headers),
//! so the fixtures are faithful ones: small stripped C programs (built with
//! `cc -s`, so they are real ELF executables) that embed exactly the marker
//! strings the frameworks embed and behave like them: GoogleTest-style
//! fixtures read `GTEST_TOTAL_SHARDS`, `GTEST_SHARD_INDEX` and touch
//! `GTEST_SHARD_STATUS_FILE`; Catch2-style ones parse `--shard-count` and
//! `--shard-index`, or reject them with Catch2's command-line error. One
//! ignored test (run by CI) builds the real frameworks instead, so these
//! fixtures cannot drift from them.
#![cfg(unix)]
#![allow(clippy::format_collect)] // building small fixture sources

mod common;

use std::collections::BTreeMap;
use std::path::Path;

/// Two configured hosts that are both the fake local one.
fn two_hosts() -> common::World {
    let w = common::world();
    let mut config = std::fs::read_to_string(w.config.join("config.toml")).unwrap();
    config = config.replace("name = \"local\"", "name = \"alpha\"");
    config.push_str("\n[[host]]\nname = \"beta\"\naddress = \"127.0.0.1\"\n");
    std::fs::write(w.config.join("config.toml"), config).unwrap();
    w
}

const SHARDED_BODY: &str = r#"
int main(int argc, char **argv) {
  const char *total = getenv("GTEST_TOTAL_SHARDS"), *idx = getenv("GTEST_SHARD_INDEX");
  const char *status = getenv("GTEST_SHARD_STATUS_FILE");
  int t = total ? atoi(total) : 1, i = idx ? atoi(idx) : 0;
  for (int a = 1; a + 1 < argc; a++) {
    if (!strcmp(argv[a], "--shard-count")) t = atoi(argv[a + 1]);
    if (!strcmp(argv[a], "--shard-index")) i = atoi(argv[a + 1]);
  }
  if (!HONOUR_STATUS) { t = 1; i = 0; }
  if (t > 1 && status) { FILE *f = fopen(status, "w"); if (f) fclose(f); }
  for (int k = 0; k < 6; k++) if (k % t == i) printf("RUN test_%d\n", k);
  return 0;
}
"#;

const GTEST: &[&str] = &[
    "GTEST_SHARD_INDEX",
    "GTEST_TOTAL_SHARDS",
    "--gtest_list_tests",
    "--gtest_filter",
];
const CATCH2: &[&str] = &[
    "--shard-count",
    "--shard-index",
    "--list-tests",
    "catch2-version",
];

/// Compile a stripped fixture embedding `markers` and the given `body`.
fn build(dir: &Path, name: &str, markers: &[&str], body: &str, defines: &[String]) {
    let list: String = markers.iter().map(|m| format!("\"{m}\",")).collect();
    let src = format!(
        "#include <stdio.h>\n#include <stdlib.h>\n#include <string.h>\n\
         const char *const embedded_markers[] = {{{list}0}};\n{body}"
    );
    std::fs::create_dir_all(dir).unwrap();
    let c = dir.join(format!("{name}.c"));
    std::fs::write(&c, src).unwrap();
    let mut cmd = std::process::Command::new("cc");
    cmd.args(["-O0", "-s", "-o"]).arg(dir.join(name)).arg(&c);
    for d in defines {
        cmd.arg(d);
    }
    let out = cmd
        .output()
        .expect("a C compiler is needed for the fixtures");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    std::fs::remove_file(c).unwrap();
}

/// A body that ignores every sharding setting and runs all tests.
const PLAIN_BODY: &str = r#"
int main(void) {
  for (int k = 0; k < 6; k++) printf("RUN test_%d\n", k);
  return 0;
}
"#;

fn sharded(honour: bool) -> String {
    SHARDED_BODY.replace("HONOUR_STATUS", if honour { "1" } else { "0" })
}

/// Counts of `RUN test_k` lines (host prefix stripped) in `stdout`.
fn runs(stdout: &str) -> BTreeMap<String, usize> {
    let mut seen = BTreeMap::new();
    for line in stdout.lines().filter(|l| l.contains("RUN test_")) {
        let line = line.split_once("] ").map_or(line, |(_, r)| r);
        *seen.entry(line.trim().to_owned()).or_insert(0) += 1;
    }
    seen
}

struct Ran {
    out: std::process::Output,
    stdout: String,
    stderr: String,
    report: serde_json::Value,
}

fn shard_run(w: &common::World, extra: &[&str], command: &[&str]) -> Ran {
    let report = w.root.join(format!("report-{}.json", std::process::id()));
    let _ = std::fs::remove_file(&report);
    let mut args = vec!["run", "--shard", "2", "--report", report.to_str().unwrap()];
    args.extend_from_slice(extra);
    args.push("--");
    args.extend_from_slice(command);
    let out = w.run(&args);
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    let report = std::fs::read_to_string(&report).map_or(serde_json::Value::Null, |t| {
        serde_json::from_str(&t).unwrap()
    });
    Ran {
        out,
        stdout,
        stderr,
        report,
    }
}

fn detections(r: &Ran) -> Vec<String> {
    r.report["shards"]
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .map(|s| {
            s["detection"]["detected"]
                .as_str()
                .unwrap_or("absent")
                .to_owned()
        })
        .collect()
}

fn exactly_once(seen: &BTreeMap<String, usize>) {
    assert_eq!(seen.len(), 6, "{seen:?}");
    assert!(seen.values().all(|&n| n == 1), "{seen:?}");
}

// frob:tests crates/goway/src/runners.rs::plan
#[test]
fn a_stripped_googletest_binary_is_sharded_natively() {
    let w = two_hosts();
    build(&w.repo.join("build"), "tests", GTEST, &sharded(true), &[]);
    let r = shard_run(&w, &[], &["./build/tests"]);
    assert_eq!(r.out.status.code(), Some(0), "{}", r.stderr);
    exactly_once(&runs(&r.stdout));
    assert_eq!(detections(&r), ["gtest", "gtest"]);
    assert!(r.stderr.contains("GoogleTest binary"), "{}", r.stderr);
}

// frob:tests crates/goway/src/runners.rs::plan
#[test]
fn a_stripped_catch2_v3_binary_gets_the_shard_flags() {
    let w = two_hosts();
    build(&w.repo.join("build"), "tests", CATCH2, &sharded(true), &[]);
    let r = shard_run(&w, &[], &["./build/tests"]);
    assert_eq!(r.out.status.code(), Some(0), "{}", r.stderr);
    exactly_once(&runs(&r.stdout));
    assert_eq!(detections(&r), ["catch2", "catch2"]);
    assert!(r.stderr.contains("Catch2 v3 binary"), "{}", r.stderr);
}

// frob:tests crates/goway/src/runners.rs::plan
#[test]
fn catch2_v2_and_partial_marker_binaries_fall_back_to_goway_shard() {
    let w = two_hosts();
    // Catch2 v2 has --list-tests but no shard flags and a different XML root.
    build(
        &w.repo.join("build"),
        "v2",
        &["--list-tests", "Catch"],
        PLAIN_BODY,
        &[],
    );
    // GoogleTest-like but missing GTEST_TOTAL_SHARDS.
    build(
        &w.repo.join("build"),
        "partial",
        &["GTEST_SHARD_INDEX", "--gtest_list_tests", "--gtest_filter"],
        PLAIN_BODY,
        &[],
    );
    for program in ["./build/v2", "./build/partial"] {
        let r = shard_run(&w, &[], &[program]);
        assert_eq!(r.out.status.code(), Some(0), "{program}: {}", r.stderr);
        // Not sharded: both shards run every test (they only see GOWAY_SHARD).
        let seen = runs(&r.stdout);
        assert_eq!(seen.len(), 6, "{program}: {seen:?}");
        assert!(seen.values().all(|&n| n == 2), "{program}: {seen:?}");
        assert_eq!(detections(&r), ["none", "none"], "{program}");
    }
}

// frob:tests crates/goway/src/runners.rs::plan
#[test]
fn scripts_and_goways_own_binary_are_never_detected() {
    let w = two_hosts();
    // A script carrying every marker of both frameworks is not an executable image.
    let all: Vec<&str> = GTEST.iter().chain(CATCH2).copied().collect();
    let script = format!("#!/bin/sh\n# {}\necho RUN test_script\n", all.join(" "));
    std::fs::create_dir_all(w.repo.join("build")).unwrap();
    let path = w.repo.join("build/script");
    std::fs::write(&path, script).unwrap();
    std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let r = shard_run(&w, &[], &["./build/script"]);
    assert_eq!(detections(&r), ["none", "none"], "{}", r.stderr);
    assert!(r.stderr.is_empty() || !r.stderr.contains("detected by reading"));
    // goway's own binary embeds the frameworks' flag names (in remote.sh).
    std::fs::copy(env!("CARGO_BIN_EXE_goway"), w.repo.join("build/goway-copy")).unwrap();
    let r = shard_run(&w, &[], &["./build/goway-copy", "--version"]);
    assert_eq!(detections(&r), ["none", "none"], "{}", r.stderr);
    assert!(!r.stderr.contains("binary (detected"), "{}", r.stderr);
}

// frob:tests crates/goway/src/runners.rs::plan
#[test]
fn goway_runner_overrides_detection() {
    let w = two_hosts();
    // Nothing in the file says GoogleTest, but the user did.
    build(
        &w.repo.join("build"),
        "tests",
        &["nothing"],
        &sharded(true),
        &[],
    );
    let r = shard_run(&w, &["--env", "GOWAY_RUNNER=gtest"], &["./build/tests"]);
    assert_eq!(r.out.status.code(), Some(0), "{}", r.stderr);
    exactly_once(&runs(&r.stdout));
    assert_eq!(
        detections(&r),
        ["absent", "absent"],
        "no detection was asked for"
    );
}

// frob:tests crates/goway/src/shard.rs::run_sharded
#[test]
fn googletest_that_ignores_sharding_is_warned_not_rerun_and_remembered() {
    let w = two_hosts();
    let log = w.root.join("invocations");
    let define = format!("-DCOUNT_FILE=\"{}\"", log.display());
    let body = format!(
        "#include <stdio.h>\nstatic void count(void) {{ FILE *f = fopen(COUNT_FILE, \"a\"); fputs(\"x\\n\", f); fclose(f); }}\n{}",
        sharded(false).replace(
            "int main(int argc, char **argv) {",
            "int main(int argc, char **argv) { count();"
        )
    );
    build(&w.repo.join("build"), "tests", GTEST, &body, &[define]);
    let r = shard_run(&w, &[], &["./build/tests"]);
    // Both shards ran the whole suite once each; nothing was rerun.
    assert_eq!(r.out.status.code(), Some(0), "{}", r.stderr);
    assert_eq!(std::fs::read_to_string(&log).unwrap().lines().count(), 2);
    let seen = runs(&r.stdout);
    assert!(seen.values().all(|&n| n == 2), "{seen:?}");
    assert!(r.stderr.contains("did not apply sharding"), "{}", r.stderr);
    assert!(r.stderr.contains("duplicated"), "{}", r.stderr);
    assert_eq!(r.report["shards"][0]["detection"]["duplicated"], true);
    // The next run of the same program skips detection, with a note.
    let r = shard_run(&w, &[], &["./build/tests"]);
    assert!(r.stderr.contains("failed before"), "{}", r.stderr);
    assert_eq!(detections(&r), ["absent", "absent"]);
    // GOWAY_RUNNER=gtest overrides the memory ...
    let r = shard_run(&w, &["--env", "GOWAY_RUNNER=gtest"], &["./build/tests"]);
    assert!(!r.stderr.contains("failed before"), "{}", r.stderr);
    // ... and `gc --repo` clears it.
    let out = w.run(&["gc", "--repo", "proj"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let r = shard_run(&w, &[], &["./build/tests"]);
    assert!(!r.stderr.contains("failed before"), "{}", r.stderr);
    assert_eq!(detections(&r), ["gtest", "gtest"]);
}

// frob:tests crates/goway/src/shard.rs::run_sharded
#[test]
fn a_catch2_binary_that_always_rejects_is_rerun_exactly_once() {
    let w = two_hosts();
    let log = w.root.join("invocations");
    let define = format!("-DCOUNT_FILE=\"{}\"", log.display());
    let body = r#"
int main(int argc, char **argv) {
  FILE *f = fopen(COUNT_FILE, "a");
  for (int a = 1; a < argc; a++) fprintf(f, "%s ", argv[a]);
  fputs("|\n", f);
  fclose(f);
  fputs("\nError(s) in input:\n  Unrecognised token: --shard-count\n\nFor more usage information run with --help\n", stderr);
  return 1;
}
"#;
    build(&w.repo.join("build"), "tests", CATCH2, body, &[define]);
    let r = shard_run(&w, &[], &["./build/tests"]);
    assert_ne!(r.out.status.code(), Some(0));
    let log_text = std::fs::read_to_string(&log).unwrap();
    let lines: Vec<&str> = log_text.lines().collect();
    // Two shards, each: one attempt with the shard flags, ONE rerun without.
    assert_eq!(lines.len(), 4, "{log_text}");
    assert_eq!(
        lines.iter().filter(|l| l.contains("--shard-count")).count(),
        2
    );
    assert_eq!(lines.iter().filter(|l| **l == "|").count(), 2, "{log_text}");
    for s in 0..2 {
        let d = &r.report["shards"][s]["detection"];
        assert_eq!(d["detected"], "catch2");
        assert_eq!(d["rerun"], true);
        assert_eq!(d["rejected_again"], true);
        assert_eq!(d["attempts"].as_array().unwrap().len(), 2);
    }
    assert!(r.stderr.contains("rejected again"), "{}", r.stderr);
    // The failure is remembered: the next run is one plain attempt per shard.
    let before = lines.len();
    let r = shard_run(&w, &[], &["./build/tests"]);
    assert!(r.stderr.contains("failed before"), "{}", r.stderr);
    let after = std::fs::read_to_string(&log).unwrap().lines().count();
    assert_eq!(after - before, 2);
}

// frob:tests crates/goway/src/shard.rs::run_sharded
#[test]
fn a_catch2_binary_that_rejects_only_the_shard_flags_is_rerun_once_and_passes() {
    let w = two_hosts();
    let body = r#"
int main(int argc, char **argv) {
  for (int a = 1; a < argc; a++)
    if (!strcmp(argv[a], "--shard-count")) {
      fputs("\nError(s) in input:\n  Unrecognised token: --shard-count\n", stderr);
      return 1;
    }
  puts("RUN test_all");
  return 0;
}
"#;
    build(&w.repo.join("build"), "tests", CATCH2, body, &[]);
    let r = shard_run(&w, &[], &["./build/tests"]);
    assert_eq!(r.out.status.code(), Some(0), "{}", r.stderr);
    assert_eq!(runs(&r.stdout).get("RUN test_all"), Some(&2));
    for s in 0..2 {
        let d = &r.report["shards"][s]["detection"];
        assert_eq!(
            (d["rerun"].clone(), d["rejected_again"].clone()),
            (true.into(), false.into())
        );
        assert_eq!(d["attempts"], serde_json::json!([1, 0]));
    }
}

// frob:tests crates/goway/src/shard.rs::run_sharded
#[test]
fn an_uncertain_failure_is_flagged_and_never_rerun() {
    let w = two_hosts();
    let log = w.root.join("invocations");
    let define = format!("-DCOUNT_FILE=\"{}\"", log.display());
    // A command-line error text, but not the shard flags' and not first.
    let body = r#"
int main(int argc, char **argv) {
  FILE *f = fopen(COUNT_FILE, "a"); fputs("x\n", f); fclose(f);
  fputs("some output first\nError(s) in input:\n  Unrecognised token: --other\n", stderr);
  return 1;
}
"#;
    build(&w.repo.join("build"), "tests", CATCH2, body, &[define]);
    let r = shard_run(&w, &[], &["./build/tests"]);
    assert_ne!(r.out.status.code(), Some(0));
    assert_eq!(std::fs::read_to_string(&log).unwrap().lines().count(), 2);
    assert_eq!(r.report["shards"][0]["detection"]["flagged"], true);
    assert_eq!(r.report["shards"][0]["detection"]["rerun"], false);
    assert!(r.stderr.contains("not certain"), "{}", r.stderr);
    // Flagged, not remembered.
    let r = shard_run(&w, &[], &["./build/tests"]);
    assert!(!r.stderr.contains("failed before"), "{}", r.stderr);
}

/// Run `cmd`, panicking with its output on failure.
fn must(cmd: &mut std::process::Command) {
    let out = cmd.output().expect("tool missing");
    assert!(
        out.status.success(),
        "{cmd:?}: {}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Distinct `RAN <prefix>_k` lines (host prefix stripped) with their counts.
fn ran_lines(stdout: &str, prefix: &str) -> BTreeMap<String, usize> {
    let mut seen = BTreeMap::new();
    for line in stdout.lines().filter(|l| l.contains("RAN ")) {
        let line = line.split_once("] ").map_or(line, |(_, r)| r).trim();
        if line.starts_with(&format!("RAN {prefix}")) {
            *seen.entry(line.to_owned()).or_insert(0) += 1;
        }
    }
    seen
}

// frob:tests crates/goway/src/runners.rs::plan
// Real frameworks, so the marker fixtures above can never drift from them:
// needs cmake, a C++ compiler, git/network (FetchContent), so it is ignored
// by default and CI runs it with `--run-ignored only`.
#[test]
#[ignore = "needs cmake, a C++ compiler and network; run by CI"]
fn real_googletest_and_catch2_binaries_are_detected_and_sharded_disjointly() {
    let w = two_hosts();
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/cxxshard");
    // Built outside the synced repository: the FetchContent trees are huge.
    let build_dir = w.root.join("cxx-build");
    must(
        std::process::Command::new("cmake")
            .arg("-S")
            .arg(&src)
            .arg("-B")
            .arg(&build_dir)
            .arg("-DCMAKE_BUILD_TYPE=Release"),
    );
    must(
        std::process::Command::new("cmake")
            .arg("--build")
            .arg(&build_dir)
            .args(["-j", "4"]),
    );
    std::fs::create_dir_all(w.repo.join("build")).unwrap();
    for (name, prefix, kind) in [("gt", "gt", "gtest"), ("c2", "c2_", "catch2")] {
        let dest = w.repo.join("build").join(name);
        std::fs::copy(build_dir.join(name), &dest).unwrap();
        must(std::process::Command::new("strip").arg(&dest));
        let r = shard_run(&w, &[], &[&format!("./build/{name}")]);
        assert_eq!(r.out.status.code(), Some(0), "{name}: {}", r.stderr);
        assert_eq!(detections(&r), [kind, kind], "{name}: {}", r.stderr);
        let seen = ran_lines(&r.stdout, prefix);
        assert_eq!(seen.len(), 12, "{name}: {seen:?}");
        assert!(seen.values().all(|&n| n == 1), "{name}: {seen:?}");
    }
}
