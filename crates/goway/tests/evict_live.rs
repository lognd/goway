//! Liveness is decided by locks and live processes, never by age: a wave of
//! concurrent runs on a host far over its disk budget never loses a live
//! run's work dir, a failed run still writes its report, and the lifeline
//! stops a job only for a reason it reports.
#![cfg(unix)]

mod common;

use std::process::Stdio;

const SCRIPT: &str = include_str!("../src/remote.sh");

/// A budget of 1 KiB: every run's end evicts whatever it can take.
fn tiny_budget(w: &common::World) {
    let cfg = w.config.join("config.toml");
    let text = std::fs::read_to_string(&cfg).unwrap();
    std::fs::write(
        &cfg,
        text.replace(
            "[defaults]\n",
            "[defaults]\nmax_disk = \"1K\"\nmin_free = \"900G\"\n",
        ),
    )
    .unwrap();
}

// frob:ticket 01M43GWXKVF2RFVTAHR1WYK8J5
// frob:tests crates/goway/src/run.rs::run
#[test]
fn a_wave_over_a_tiny_budget_never_loses_a_live_runs_work_dir() {
    let w = common::world();
    tiny_budget(&w);
    // Each run proves its work dir, TMPDIR and cache survive while the others
    // start and finish, and that the budget really evicts (a later run reports it).
    let script = "i=0; while [ $i -lt 40 ]; do [ -d \"$TMPDIR\" ] || { echo LOST; exit 7; }; echo x >\"$TMPDIR/f$i\" || { echo LOST; exit 7; }; i=$((i+1)); sleep 0.1; done; echo ok";
    let kids: Vec<_> = (0..6u64)
        .map(|n| {
            std::thread::sleep(std::time::Duration::from_millis(400 * n));
            let mut c = w.goway(&["run", "--host", "local", "--", "sh", "-c", script]);
            c.stdout(Stdio::piped()).stderr(Stdio::piped());
            c.spawn().unwrap()
        })
        .collect();
    let mut evictions = 0;
    for k in kids {
        let out = k.wait_with_output().unwrap();
        let o = String::from_utf8_lossy(&out.stdout).into_owned();
        let e = String::from_utf8_lossy(&out.stderr).into_owned();
        evictions += e.matches("disk budget: evicted").count();
        assert!(out.status.success() && o.contains("ok"), "{o}\n{e}");
    }
    let tail = w.run(&["run", "--host", "local", "--", "true"]);
    evictions += String::from_utf8_lossy(&tail.stderr)
        .matches("disk budget: evicted")
        .count();
    assert!(
        evictions > 0,
        "the budget never evicted: the test proves nothing"
    );
}

// frob:ticket 01M43GWXKVF2RFVTAHR1WYK8J5
// frob:tests crates/goway/src/run.rs::run
#[test]
fn a_failed_run_still_writes_its_report() {
    let w = common::world();
    let report = w.root.join("report.json");
    let out = w.run(&[
        "run",
        "--host",
        "local",
        "--report",
        report.to_str().unwrap(),
        "--",
        "sh",
        "-c",
        "exit 3",
    ]);
    assert_eq!(out.status.code(), Some(3));
    let json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&report).unwrap()).unwrap();
    assert_eq!(json["exit_code"], 3);
    // A run that fails before any command ran (no such host) leaves one too.
    let early = w.root.join("early.json");
    let out = w.run(&[
        "run",
        "--host",
        "nowhere",
        "--report",
        early.to_str().unwrap(),
        "--",
        "true",
    ]);
    assert_eq!(out.status.code(), Some(125));
    let json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&early).unwrap()).unwrap();
    assert_eq!(json["exit_code"], 125);
    assert!(json["error"].as_str().is_some_and(|e| !e.is_empty()));
}

/// Run the remote script's `lifeline` for run `id` with `input` on stdin.
fn lifeline(w: &common::World, id: &str, input: &str, timeout: &str) -> std::process::Output {
    use std::io::Write as _;
    let mut child = std::process::Command::new("bash")
        .args(["-c", SCRIPT, "goway", "lifeline"])
        .arg(&w.remote)
        .arg(id)
        .env("HOME", &w.root)
        .env("GOWAY_LIFELINE_TIMEOUT", timeout)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    stdin.write_all(input.as_bytes()).unwrap();
    if input == "hold" {
        // Silence: keep the pipe open and say nothing.
        std::mem::forget(stdin);
    } else {
        drop(stdin);
    }
    child.wait_with_output().unwrap()
}

// frob:ticket 01M43GWXKVF2RFVTAHR1WYK8J5
// frob:tests crates/goway/src/run.rs::stream
#[test]
fn the_lifeline_says_why_it_stopped_a_job_and_tells_silence_from_a_closed_pipe() {
    let w = common::world();
    // A marked root with a live-looking run.
    assert!(
        std::process::Command::new("bash")
            .args(["-c", SCRIPT, "goway", "manifest"])
            .arg(&w.remote)
            .arg("abc")
            .env("HOME", &w.root)
            .output()
            .unwrap()
            .status
            .success()
    );
    for id in ["closed", "silent"] {
        std::fs::create_dir_all(w.remote.join("work").join(id)).unwrap();
    }
    // End of input: the client is certainly gone; the reason is recorded.
    let out = lifeline(&w, "closed", "", "30");
    let why = std::fs::read_to_string(w.remote.join("work/closed/lost")).unwrap();
    assert!(why.contains("connection closed"), "{why} {out:?}");
    // Silence stops only after the whole window, with its own reason.
    let started = std::time::Instant::now();
    let _ = lifeline(&w, "silent", "hold", "2");
    assert!(started.elapsed() >= std::time::Duration::from_secs(2));
    let why = std::fs::read_to_string(w.remote.join("work/silent/lost")).unwrap();
    assert!(why.contains("silent for 2s"), "{why}");
}
