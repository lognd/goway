//! Sleep inhibitor: a job holds one for exactly its lifetime, through the
//! fake-ssh world with a fake `systemd-inhibit` on the PATH.
#![cfg(unix)]

mod common;

use std::os::unix::fs::PermissionsExt as _;

/// Put an executable `name` with `body` in the world's bin directory.
fn fake(w: &common::World, name: &str, body: &str) {
    let path = w.bin.join(name);
    std::fs::write(&path, body).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// A world whose `systemd-inhibit` creates `held` while the wrapped command runs
/// (the probe `true` is run plainly, so it never leaves the marker behind).
fn inhibit_world() -> (common::World, std::path::PathBuf) {
    let w = common::world();
    let held = w.root.join("held");
    fake(
        &w,
        "systemd-inhibit",
        &format!(
            "#!/bin/sh\nwhile [ $# -gt 0 ]; do case \"$1\" in --*) shift ;; *) break ;; esac; done\n\
             [ \"$1\" = true ] && exec \"$@\"\n\
             : > '{held}'; \"$@\"; rc=$?; rm -f '{held}'; exit $rc\n",
            held = held.display()
        ),
    );
    (w, held)
}

fn out(o: &std::process::Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

// frob:tests crates/goway/src/run.rs::run_invocation_with
#[test]
fn a_job_holds_the_inhibitor_exactly_while_it_runs() {
    let (w, held) = inhibit_world();
    let probe = format!(
        "if [ -e '{}' ]; then echo HELD=yes; else echo HELD=no; fi",
        held.display()
    );
    let o = w.run(&["run", "--", "sh", "-c", &probe]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert!(out(&o).contains("HELD=yes"), "{}", out(&o));
    assert!(!held.exists(), "the inhibitor outlived the job");
}

// frob:tests crates/goway/src/run.rs::run_invocation_with
#[test]
fn the_inhibitor_is_released_when_the_job_fails_and_the_exit_code_is_the_jobs() {
    let (w, held) = inhibit_world();
    let o = w.run(&["run", "--", "sh", "-c", "exit 3"]);
    assert_eq!(
        o.status.code(),
        Some(3),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    assert!(!held.exists(), "the inhibitor outlived the failed job");
}

// frob:tests crates/goway/src/run.rs::run_invocation_with
#[test]
fn a_host_whose_inhibitor_does_not_work_still_runs_the_job() {
    let w = common::world();
    fake(
        &w,
        "systemd-inhibit",
        "#!/bin/sh\necho 'Failed to inhibit' >&2\nexit 1\n",
    );
    let o = w.run(&["run", "--", "sh", "-c", "echo ran"]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert!(out(&o).contains("ran"));
}
