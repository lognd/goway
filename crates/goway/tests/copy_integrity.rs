//! Copy integrity: the helper's copy of the tree is verified before the
//! command starts and again when it fails; a proven mismatch rebuilds the
//! copy and reruns exactly once, and is remembered locally.
#![cfg(unix)]

mod common;

use std::path::Path;

use common::{World, world};

/// A goway command whose fake helper falsifies its verification claims.
fn corrupt(w: &World, mode: &str, args: &[&str]) -> std::process::Output {
    w.goway(args)
        .env(
            "GOWAY_SSH_PASS_ENV",
            "FAKE_HOSTNAME,FAKE_WINDOWS_PORT,RUSTC_WRAPPER,CARGO_TARGET_DIR,GOWAY_WINDOWS_LOOKUP,GOWAY_TEST_CORRUPT",
        )
        .env("GOWAY_TEST_CORRUPT", mode)
        .output()
        .unwrap()
}

fn report(path: &Path) -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn stderr(out: &std::process::Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[test]
fn every_run_verifies_what_it_wrote_before_the_command_starts() {
    let w = world();
    let out = w.run(&["run", "--keep", "--", "cat", "hello.txt"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let dir = w.remote.join("work").join(&w.work_dirs()[0]);
    assert_eq!(
        std::fs::read_to_string(dir.join("verdict.1")).unwrap(),
        "ok"
    );
    let claims = std::fs::read(dir.join("verify.1")).unwrap();
    assert!(claims.starts_with(b"goway-verify1\0f\x01"), "{claims:?}");
    assert!(claims.windows(9).any(|w| w == b"hello.txt"));
    assert!(
        !dir.join("verify.2").exists(),
        "no failure, no second check"
    );
}

#[test]
fn a_genuine_failure_is_verified_and_never_rerun() {
    let w = world();
    let rep = w.root.join("r.json");
    let out = w.run(&[
        "run",
        "--keep",
        "--report",
        rep.to_str().unwrap(),
        "--",
        "sh",
        "-c",
        "echo ran; exit 3",
    ]);
    assert_eq!(out.status.code(), Some(3), "{}", stderr(&out));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "ran\n", "ran once");
    let r = report(&rep);
    let attempts = r["attempts"].as_array().unwrap();
    assert_eq!(attempts.len(), 1, "{r}");
    assert_eq!(attempts[0]["valid"], true);
    assert_eq!(attempts[0]["exit_code"], 3);
    let dir = w.remote.join("work").join(&w.work_dirs()[0]);
    assert_eq!(
        std::fs::read_to_string(dir.join("verdict.2")).unwrap(),
        "ok"
    );
}

#[test]
fn files_a_failing_command_changed_itself_are_not_blamed_on_the_copy() {
    let w = world();
    let out = w.run(&["run", "--", "sh", "-c", "echo changed >> hello.txt; exit 4"]);
    assert_eq!(out.status.code(), Some(4), "{}", stderr(&out));
    assert!(!stderr(&out).contains("goway bug"), "{}", stderr(&out));
}

#[test]
fn a_bad_copy_stops_the_command_reruns_once_and_is_remembered() {
    let w = world();
    let rep = w.root.join("r.json");
    let out = corrupt(
        &w,
        "first",
        &[
            "run",
            "--report",
            rep.to_str().unwrap(),
            "--",
            "sh",
            "-c",
            "echo ran",
        ],
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "ran\n",
        "the command ran once"
    );
    let text = stderr(&out);
    assert!(text.contains("goway bug"), "{text}");
    assert!(text.contains("repository id"), "{text}");
    assert!(text.contains("hello.txt"), "{text}");
    let r = report(&rep);
    let attempts = r["attempts"].as_array().unwrap();
    assert_eq!(attempts.len(), 2, "{r}");
    assert_eq!(attempts[0]["valid"], false);
    assert_eq!(attempts[1]["valid"], true);
    assert_eq!(attempts[0]["mismatched"][0], "hello.txt");

    // Remembered locally: full verification on a fresh slot, until cleared.
    let again = w.run(&["run", "--", "true"]);
    assert!(
        stderr(&again).contains("had a copy mismatch lately"),
        "{}",
        stderr(&again)
    );
    let trusted = w.run(&["run", "--trust-copy", "--", "true"]);
    assert!(!stderr(&trusted).contains("had a copy mismatch lately"));
    assert!(w.run(&["gc", "--repo", "proj"]).status.success());
    let cleared = w.run(&["run", "--", "true"]);
    assert!(!stderr(&cleared).contains("had a copy mismatch lately"));
    let state = std::fs::read_to_string(w.root.join("state/hosts.json")).unwrap();
    assert!(
        !state.contains("distrust"),
        "the mark lives in local state only: {state}"
    );
    assert!(
        !std::fs::read_to_string(w.config.join("config.toml"))
            .unwrap()
            .contains("distrust")
    );
}

#[test]
fn a_failure_with_a_bad_copy_is_rerun_once_on_a_rebuilt_copy() {
    let w = world();
    let rep = w.root.join("r.json");
    let out = corrupt(
        &w,
        "after",
        &[
            "run",
            "--report",
            rep.to_str().unwrap(),
            "--",
            "sh",
            "-c",
            "echo ran; exit 3",
        ],
    );
    assert_eq!(out.status.code(), Some(3), "{}", stderr(&out));
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "ran\nran\n",
        "twice: bad copy, rebuilt copy"
    );
    let r = report(&rep);
    let attempts = r["attempts"].as_array().unwrap();
    assert_eq!(attempts.len(), 2);
    assert_eq!(attempts[0]["valid"], false);
    assert_eq!(attempts[1]["valid"], true);
    assert_eq!(attempts[1]["exit_code"], 3);
}

/// A helper that always reports a mismatch: exactly two attempts, then 125.
#[test]
fn a_helper_that_always_mismatches_gets_exactly_two_attempts_then_125() {
    let w = world();
    let rep = w.root.join("r.json");
    let out = corrupt(
        &w,
        "1",
        &[
            "run",
            "--report",
            rep.to_str().unwrap(),
            "--",
            "sh",
            "-c",
            "echo ran",
        ],
    );
    assert_eq!(out.status.code(), Some(125), "{}", stderr(&out));
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "",
        "the command never started"
    );
    let r = report(&rep);
    let attempts = r["attempts"].as_array().unwrap();
    assert_eq!(attempts.len(), 2, "{r}");
    assert!(attempts.iter().all(|a| a["valid"] == false));
    assert_eq!(attempts[0]["attempt"], 1);
    assert_eq!(attempts[1]["attempt"], 2);
    assert_eq!(r["exit_code"], 125);
    assert!(
        stderr(&out).contains("goway stops here"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn every_shard_is_verified_and_reruns_at_most_once() {
    let w = world();
    let mut config = std::fs::read_to_string(w.config.join("config.toml")).unwrap();
    config = config.replace("name = \"local\"", "name = \"alpha\"");
    config.push_str("\n[[host]]\nname = \"beta\"\naddress = \"127.0.0.1\"\n");
    std::fs::write(w.config.join("config.toml"), config).unwrap();
    let rep = w.root.join("s.json");
    let out = corrupt(
        &w,
        "1",
        &[
            "run",
            "--shard",
            "2",
            "--report",
            rep.to_str().unwrap(),
            "--",
            "sh",
            "-c",
            "echo ran",
        ],
    );
    assert_eq!(out.status.code(), Some(125), "{}", stderr(&out));
    let r = report(&rep);
    for shard in r["shards"].as_array().unwrap() {
        assert_eq!(shard["attempts"].as_array().unwrap().len(), 2, "{shard}");
        assert_eq!(shard["exit_code"], 125);
    }
}
