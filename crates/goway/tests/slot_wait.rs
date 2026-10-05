//! The helper-side wait for a build slot is bounded by the run's `--wait`: it says how many
//! slots are busy and how long the oldest holder has run, and gives up with exit 125.
#![cfg(unix)]

mod common;

// frob:ticket 01M44F0H5R09BNX6PJWD2QQV2Q
// frob:tests crates/goway/src/run.rs::slot_wait_word
#[test]
fn a_run_waiting_for_a_busy_slot_gives_up_after_wait_with_what_it_saw() {
    let w = common::world();
    // The world has two build slots; two held runs take both.
    let a = w.hold(&[], "true");
    a.wait_started();
    let b = w.hold(&[], "true");
    b.wait_started();
    let started = std::time::Instant::now();
    let out = w.run(&["run", "--wait", "2s", "--", "true"]);
    let err = String::from_utf8_lossy(&out.stderr).into_owned();
    assert_eq!(out.status.code(), Some(125), "{err}");
    assert!(err.contains("all 2 build slots busy"), "{err}");
    assert!(err.contains("oldest holder has run"), "{err}");
    assert!(err.contains("no build slot freed within 2s"), "{err}");
    assert!(
        started.elapsed() < std::time::Duration::from_secs(60),
        "the wait was not bounded"
    );
    // The give-up leaves no work dir of its own behind.
    assert_eq!(w.work_dirs().len(), 2, "{:?}", w.work_dirs());
    a.finish();
    b.finish();
}
