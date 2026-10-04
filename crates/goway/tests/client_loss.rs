//! A client that vanishes without the connection hanging up (SIGKILL, a
//! sleeping laptop, a dropped network) must not leave its job running on the
//! helper: the helper's lifeline stops it within a bound.
#![cfg(unix)]

mod common;

use std::time::{Duration, Instant};

/// Like the real ssh client: it is a separate process that stays alive when
/// goway is killed, so the remote shell's parent never dies and the plain
/// parent-liveness watchdog cannot notice.
const SURVIVING_SSH: &str = r#"#!/bin/sh
while [ $# -gt 0 ]; do
  case "$1" in
    -o|-p|-l) shift 2 ;;
    --) shift; shift; break ;;
    *) shift ;;
  esac
done
sh -c "$1"
"#;

/// The longest the helper may take to stop the job after a SIGKILL'd client
/// (end of stdin is seen at once; the rest is the stop's own grace).
const BOUND: Duration = Duration::from_secs(20);

fn alive(pid: u32) -> bool {
    std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

// frob:tests crates/goway/src/run.rs::stream
#[test]
fn a_sigkilled_client_stops_its_job_within_the_bound_and_frees_the_work_dir() {
    let w = common::world_with_ssh(SURVIVING_SSH);
    let pidfile = w.root.join("job.pid");
    let mut held = w.hold(&[], &format!("echo $$ > '{}'", pidfile.display()));
    held.wait_started();
    let pid: u32 = std::fs::read_to_string(&pidfile)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert!(alive(pid), "the job runs while its client lives");
    assert!(!w.work_dirs().is_empty());

    let killed_at = Instant::now();
    held.kill_client();
    common::wait_for("the orphaned job to be stopped", || !alive(pid));
    assert!(
        killed_at.elapsed() < BOUND,
        "stopped after {:?}",
        killed_at.elapsed()
    );
    common::wait_for("the run's work dir to be cleaned", || {
        w.work_dirs().is_empty()
    });
}
