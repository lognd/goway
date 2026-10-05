//! A client that vanishes without the connection hanging up (SIGKILL, a
//! sleeping laptop, a dropped network) must not leave its job running on the
//! helper: the helper's lifeline stops it within a bound.
#![cfg(unix)]

mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
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
    // kill -0 (no /proc on macOS), and a zombie waiting for its reaper counts as gone.
    let signalled = std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    signalled
        && !std::process::Command::new("ps")
            .args(["-o", "stat=", "-p", &pid.to_string()])
            .output()
            .is_ok_and(|o| String::from_utf8_lossy(&o.stdout).trim().starts_with('Z'))
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

/// Pids recorded in `file`, one per line.
fn pids(file: &Path) -> Vec<u32> {
    std::fs::read_to_string(file)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| l.trim().parse().ok())
        .collect()
}

/// Shell that starts three background processes of a job: one in the job's group, one in its
/// own session (setsid) and a shell loop, each recording its pid in `file`.
fn stragglers(file: &Path) -> String {
    let f = file.display();
    // A job that leaves its session is only caught where goway can find its tag (/proc).
    let escaper = if cfg!(target_os = "linux") {
        format!("setsid sh -c 'echo $$ >> {f}; exec sleep 121' & ")
    } else {
        format!("sh -c 'echo $$ >> {f}; exec sleep 121' & ")
    };
    format!(
        "sh -c 'echo $$ >> {f}; exec sleep 120' & {escaper} \
         sh -c 'echo $$ >> {f}; while :; do sleep 1; done' & true"
    )
}

/// Every build-slot lock under the world's remote root can be taken at once.
fn slot_locks_free(w: &common::World) -> bool {
    let out = std::process::Command::new("find")
        .arg(&w.remote)
        .args(["-name", "target-*.lock"])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).lines().all(|f| {
        // No flock binary (macOS): the lock cannot be probed from here.
        std::process::Command::new("flock")
            .args(["-n", f, "true"])
            .status()
            .map_or(true, |s| s.success())
    })
}

/// A `systemd-run` that always fails, so the helper has no scope (the fallback path).
fn without_scope(w: &common::World) {
    let fake = w.bin.join("systemd-run");
    std::fs::write(&fake, "#!/bin/sh\nexit 1\n").unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// Whether this machine can run a transient user scope (the scope path under test).
fn has_scopes() -> bool {
    std::process::Command::new("systemd-run")
        .args(["--user", "--scope", "--quiet", "--collect", "true"])
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// Kill the client of a held run that has stragglers; every one must end, and the slot lock free.
fn sigkilled_client_leaves_nothing(w: &common::World) {
    let file = w.root.join("stragglers.pids");
    let mut held = w.hold(&[], &format!("{} sleep 0.2", stragglers(&file)));
    held.wait_started();
    common::wait_for("the stragglers to start", || pids(&file).len() >= 3);
    let all = pids(&file);
    let killed_at = Instant::now();
    held.kill_client();
    common::wait_for("every process of the job to end", || {
        all.iter().all(|p| !alive(*p))
    });
    assert!(killed_at.elapsed() < BOUND, "{:?}", killed_at.elapsed());
    common::wait_for("the run's work dir to be cleaned", || {
        w.work_dirs().is_empty()
    });
    common::wait_for("the slot lock to be free", || slot_locks_free(w));
    for p in all {
        let _ = std::process::Command::new("kill")
            .args(["-KILL", &p.to_string()])
            .status();
    }
}

/// A job whose leader exits while its background processes run on: after the run, none remain.
fn finished_run_leaves_nothing(w: &common::World) {
    let file = w.root.join("left.pids");
    let f = file.display();
    let script = format!(
        "{}; while [ $(wc -l < {f}) -lt 3 ]; do sleep 0.1; done",
        stragglers(&file)
    );
    let out = w
        .goway(&["run", "--", "sh", "-c", &script])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap();
    assert!(out.success());
    let all = pids(&file);
    assert_eq!(all.len(), 3);
    common::wait_for("no process of the finished run to remain", || {
        all.iter().all(|p| !alive(*p))
    });
    common::wait_for("the slot lock to be free", || slot_locks_free(w));
    for p in all {
        let _ = std::process::Command::new("kill")
            .args(["-KILL", &p.to_string()])
            .status();
    }
}

// frob:ticket 01M44F3BBCMAXQNP7H2SZFHCRG
// frob:tests crates/goway/src/run.rs::stream
#[test]
fn a_sigkilled_client_leaves_no_process_of_its_job_without_a_scope() {
    let w = common::world_with_ssh(SURVIVING_SSH);
    without_scope(&w);
    sigkilled_client_leaves_nothing(&w);
}

// frob:ticket 01M44F3BBCMAXQNP7H2SZFHCRG
// frob:tests crates/goway/src/run.rs::stream
#[test]
fn a_sigkilled_client_leaves_no_process_of_its_job_in_its_scope() {
    if !has_scopes() {
        return;
    }
    let w = common::world_with_ssh(SURVIVING_SSH);
    sigkilled_client_leaves_nothing(&w);
}

// frob:ticket 01M44F3BBCMAXQNP7H2SZFHCRG
// frob:tests crates/goway/src/run.rs::stream
#[test]
fn a_finished_run_leaves_no_background_process_without_a_scope() {
    let w = common::world();
    without_scope(&w);
    finished_run_leaves_nothing(&w);
}

// frob:ticket 01M44F3BBCMAXQNP7H2SZFHCRG
// frob:tests crates/goway/src/run.rs::stream
#[test]
fn a_finished_run_leaves_no_background_process_in_its_scope() {
    if !has_scopes() {
        return;
    }
    let w = common::world();
    finished_run_leaves_nothing(&w);
}

// frob:ticket 01M44T3VY7H4QFZRRXYCDMA5DG
// frob:tests crates/goway/src/run.rs::stream
#[test]
fn a_plain_run_without_a_scope_finds_no_leftovers_of_its_own() {
    let w = common::world();
    without_scope(&w);
    let out = w.run(&["run", "--", "true"]);
    assert!(out.status.success(), "{out:?}");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(!err.contains("left processes behind"), "{err}");
}

fn have_sccache() -> bool {
    cfg!(target_os = "linux")
        && std::process::Command::new("sccache")
            .arg("--version")
            .output()
            .is_ok()
}

/// Restart the repository's sccache server from inside the job, as a build does when the
/// server idled out: the new server inherits the run's tag.
const RESTART_SERVER: &str = "sccache --stop-server >/dev/null 2>&1; \
     TMPDIR=$(cat \"$(dirname \"$SCCACHE_DIR\")/sccache.tmpdir\") sccache --start-server >/dev/null 2>&1";

// frob:ticket 01M4521XH97V8EXXK8RV630TRA
// frob:tests crates/goway/src/remote.rs::invocation
#[test]
fn the_configured_sccache_server_is_no_leftover_and_keeps_serving() {
    if !have_sccache() {
        return;
    }
    let w = common::world();
    without_scope(&w);
    let out = w.run(&["run", "--", "sh", "-c", RESTART_SERVER]);
    assert!(out.status.success(), "{out:?}");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(!err.contains("left processes behind"), "{err}");
    assert!(
        !common::sccache_servers_under(&w.root).is_empty(),
        "the run ended the server"
    );
}

// frob:ticket 01M4521XH97V8EXXK8RV630TRA
// frob:tests crates/goway/src/remote.rs::invocation
#[test]
fn a_real_stray_is_stopped_and_named_while_the_sccache_server_stays() {
    if !have_sccache() {
        return;
    }
    let w = common::world();
    without_scope(&w);
    let script = format!("{RESTART_SERVER}; setsid sleep 4711 >/dev/null 2>&1 &");
    let out = w.run(&["run", "--", "sh", "-c", &script]);
    assert!(out.status.success(), "{out:?}");
    let err = String::from_utf8_lossy(&out.stderr);
    let line = err
        .lines()
        .find(|l| l.contains("left processes behind"))
        .unwrap_or_else(|| panic!("no leftover message in {err}"));
    assert!(line.contains("pid "), "{line}");
    assert!(line.contains("(sleep 4711)"), "{line}");
    assert!(!line.contains("sccache"), "{line}");
    common::wait_for("the stray to be stopped", || {
        !common::process_mentions("sleep\u{0}4711")
    });
    assert!(!common::sccache_servers_under(&w.root).is_empty());
}

// frob:ticket 01M4521XH97V8EXXK8RV630TRA
// frob:tests crates/goway/src/remote.rs::invocation
#[test]
fn the_sweep_prints_no_permission_errors_for_processes_it_may_not_read() {
    let w = common::world();
    without_scope(&w);
    let out = w.run(&[
        "run",
        "--",
        "sh",
        "-c",
        "setsid sleep 4712 >/dev/null 2>&1 &",
    ]);
    assert!(out.status.success(), "{out:?}");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("left processes behind"), "{err}");
    assert!(!err.contains("Permission denied"), "{err}");
    assert!(!err.contains("/proc/"), "{err}");
}
