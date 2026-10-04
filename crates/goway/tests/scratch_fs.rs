//! Unusual temp and home file systems: scratch files live in goway's own
//! root, and a repository with case-only path clashes is refused on a host
//! whose file system ignores case.
#![cfg(unix)]

use std::path::Path;
use std::process::{Command, Output};

mod common;

use common::world;

const SCRIPT: &str = include_str!("../src/remote.sh");

fn remote(home: &Path, verb: &str, args: &[&str], stdin: &[u8], env: &[(&str, &str)]) -> Output {
    use std::io::Write as _;
    use std::process::Stdio;
    let mut child = Command::new("bash")
        .args(["-c", SCRIPT, "goway", verb])
        .args(args)
        .env("HOME", home)
        .envs(env.iter().copied())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(stdin).unwrap();
    child.wait_with_output().unwrap()
}

/// A tar stream holding the named empty files.
fn tar_of(names: &[&str]) -> Vec<u8> {
    let mut b = tar::Builder::new(Vec::new());
    for n in names {
        let mut h = tar::Header::new_gnu();
        h.set_size(0);
        h.set_mode(0o644);
        h.set_cksum();
        b.append_data(&mut h, n, std::io::empty()).unwrap();
    }
    b.into_inner().unwrap()
}

// frob:tests crates/goway/src/remote.rs::invocation
#[test]
fn a_run_keeps_its_scratch_files_under_the_remote_root_not_tmp() {
    let w = world();
    let out = w.run(&["run", "--", "sh", "-c", "echo $TMPDIR; touch $TMPDIR/probe"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let dir = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    assert!(
        dir.starts_with(&w.remote.join("work").display().to_string()),
        "{dir}"
    );
    assert!(dir.ends_with("/tmp"), "{dir}");
}

/// Whether the file system under `dir` treats names that differ only in case as one.
fn temp_ignores_case(dir: &std::path::Path) -> bool {
    std::fs::write(dir.join("case-probe"), b"").unwrap();
    let same = dir.join("CASE-PROBE").exists();
    std::fs::remove_file(dir.join("case-probe")).unwrap();
    same
}

// frob:tests crates/goway/src/remote.rs::invocation
#[test]
fn case_only_clashes_are_refused_on_a_case_insensitive_host_naming_both_paths() {
    let home = tempfile::tempdir().unwrap();
    let env = [("GOWAY_ASSUME_CASE_INSENSITIVE", "1")];
    let receive = |names: &[&str], env: &[(&str, &str)]| {
        let generation =
            std::fs::read_to_string(home.path().join(".cache/goway/seed/seed1/generation"))
                .unwrap_or_default();
        let args = [
            ".cache/goway",
            "seed1",
            "e30=",
            generation.trim(),
            "",
            "",
            "0",
            "",
        ];
        remote(home.path(), "receive", &args, &tar_of(names), env)
    };
    let out = receive(&["src/Main.rs", "src/main.rs"], &env);
    assert_eq!(out.status.code(), Some(76));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("src/Main.rs") && err.contains("src/main.rs"),
        "{err}"
    );
    assert!(err.contains("differ only in case"), "{err}");
    // Nothing was extracted.
    assert!(
        !home
            .path()
            .join(".cache/goway/seed/seed1/tree/src")
            .exists()
    );

    // The same stream is fine on a case-sensitive file system (not on this one, if it
    // ignores case, as APFS does by default: the host then refuses it for real).
    let out = receive(&["src/Main.rs", "src/main.rs"], &[]);
    if temp_ignores_case(home.path()) {
        assert_eq!(out.status.code(), Some(76));
        // Nothing was extracted, so put one file in the seed for the next check.
        let out = receive(&["src/main.rs"], &[]);
        assert!(out.status.success());
    } else {
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    // A clash with a file already in the seed is caught too.
    let out = receive(&["SRC/main.rs"], &env);
    assert_eq!(
        out.status.code(),
        Some(76),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

// frob:tests crates/goway/src/doctor.rs::filesystem_checks
#[test]
fn the_remote_doctor_reports_the_file_system_of_the_root_and_of_tmp() {
    let home = tempfile::tempdir().unwrap();
    let out = remote(home.path(), "doctor", &[".cache/goway"], b"", &[]);
    let text = String::from_utf8_lossy(&out.stdout);
    for key in [
        "root_fs=",
        "root_free=",
        "root_noexec=0",
        "tmp_fs=",
        "tmp_noexec=",
    ] {
        assert!(text.contains(key), "{key} missing in {text}");
    }
    let flag = format!(
        "root_case_insensitive={}",
        u8::from(temp_ignores_case(home.path()))
    );
    assert!(text.contains(&flag), "{flag} missing in {text}");
}
