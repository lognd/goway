//! End-to-end `goway run` without a network: a fake `ssh` on PATH runs the
//! remote command through a local shell, so the real binary, the real
//! remote script and the real sync protocol are exercised.
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::Command;

mod common;

use common::{FAKE_SSH, world};

#[test]
fn exit_code_passes_through_and_work_dir_is_removed() {
    let w = world();
    let out = w.run(&[
        "run",
        "--",
        "sh",
        "-c",
        "cat hello.txt; echo to-stderr >&2; exit 7",
    ]);
    assert_eq!(
        out.status.code(),
        Some(7),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout), "hello\n");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("to-stderr"), "{stderr}");
    assert!(stderr.contains("goway: info: running on local"), "{stderr}");
    assert!(w.work_dirs().is_empty(), "{:?}", w.work_dirs());

    let kept = w.run(&["run", "--keep", "--", "true"]);
    assert_eq!(kept.status.code(), Some(0));
    assert_eq!(w.work_dirs().len(), 1, "--keep keeps the work dir");
}

#[test]
fn cargo_target_dir_is_a_free_per_repo_slot() {
    let w = world();
    let print = ["run", "--", "sh", "-c", "echo $CARGO_TARGET_DIR"];
    let first = w.run(&print);
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let slot0 = String::from_utf8_lossy(&first.stdout).trim().to_owned();
    assert!(
        slot0.starts_with(&w.remote.join("cache").display().to_string()),
        "{slot0}"
    );
    assert!(slot0.ends_with("/target-0"), "{slot0}");

    // While one run holds slot 0, a concurrent run gets slot 1.
    let mut busy = w
        .goway(&["run", "--", "sh", "-c", "sleep 3"])
        .spawn()
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(1500));
    let second = w.run(&print);
    let slot = String::from_utf8_lossy(&second.stdout).trim().to_owned();
    assert!(busy.wait().unwrap().success());
    assert!(
        slot.ends_with("/target-1"),
        "{slot}\n{}",
        String::from_utf8_lossy(&second.stderr)
    );
    assert_eq!(Path::new(&slot).parent(), Path::new(&slot0).parent());
}

#[test]
fn report_names_host_arch_address_and_exit_code() {
    let w = world();
    let report = w.root.join("report.json");
    let out = w.run(&[
        "run",
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
    assert_eq!(json["host"], "local");
    assert_eq!(json["address"], "127.0.0.1");
    assert_eq!(json["exit_code"], 3);
    assert_eq!(json["arch"], std::env::consts::ARCH);
    assert!(json["hostname"].as_str().is_some_and(|h| !h.is_empty()));
}

#[test]
fn env_files_never_reach_the_remote() {
    let w = world();
    std::fs::write(w.repo.join(".env"), "SECRET=placeholder\n").unwrap();
    let out = w.run(&["run", "--", "sh", "-c", "ls -A"]);
    let listing = String::from_utf8_lossy(&out.stdout);
    assert!(listing.contains("hello.txt"), "{listing}");
    assert!(!listing.contains(".env"), "{listing}");
}

#[test]
fn signals_map_to_128_plus_n() {
    let w = world();
    let out = w.run(&["run", "--", "sh", "-c", "kill -TERM $$"]);
    assert_eq!(out.status.code(), Some(143));
}

// frob:tests crates/goway/src/status.rs::status
// frob:tests crates/goway/src/status.rs::rows
#[test]
fn status_shows_load_jobs_and_disk_and_marks_unreachable() {
    let w = world();
    // A second host that never answers: the fake ssh fails for its address.
    let mut config = std::fs::read_to_string(w.config.join("config.toml")).unwrap();
    config.push_str("\n[[host]]\nname = \"gone\"\naddress = \"10.255.255.1\"\nmax_jobs = 1\n");
    std::fs::write(w.config.join("config.toml"), config).unwrap();
    std::fs::write(
        w.bin.join("ssh"),
        FAKE_SSH.replace("exec sh -c", "[ \"$addr\" = 10.255.255.1 ] && { echo 'ssh: connect to host 10.255.255.1 port 2222: Connection timed out' >&2; exit 255; }\nexec sh -c")
            .replace("--) shift; shift; break ;;", "--) shift; addr=$1; shift; break ;;"),
    )
    .unwrap();
    assert!(
        w.run(&["run", "--host", "local", "--", "true"])
            .status
            .success()
    );
    let out = w.run(&["status"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let table = String::from_utf8_lossy(&out.stdout);
    let lines: Vec<&str> = table.lines().collect();
    assert!(lines[0].starts_with("host"), "{table}");
    let local = lines.iter().find(|l| l.starts_with("local")).unwrap();
    assert!(local.contains("127.0.0.1 (cached)"), "{local}");
    assert!(local.contains(std::env::consts::ARCH), "{local}");
    assert!(local.contains('B'), "disk shown: {local}");
    let gone = lines.iter().find(|l| l.starts_with("gone")).unwrap();
    assert!(gone.contains("unreachable"), "{gone}");
    assert!(String::from_utf8_lossy(&out.stderr).contains("gone: cannot reach host"));
}

/// Set the mtime of `path` to `days` days ago.
fn backdate(path: &Path, days: u32) {
    let ok = Command::new("touch")
        .arg("-d")
        .arg(format!("{days} days ago"))
        .arg(path)
        .status()
        .unwrap();
    assert!(ok.success());
}

fn only_dir(dir: &Path) -> PathBuf {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.is_dir())
        .collect();
    assert_eq!(dirs.len(), 1, "{dirs:?}");
    dirs.pop().unwrap()
}

// frob:tests crates/goway/src/gc.rs::gc
// frob:tests crates/goway/src/gc.rs::command
#[test]
fn gc_removes_expired_unlocked_entries_and_keeps_locked_or_fresh_ones() {
    let w = world();
    assert!(w.run(&["run", "--keep", "--", "true"]).status.success());
    let kept = w.remote.join("work").join(&w.work_dirs()[0]);
    backdate(&kept.join("meta.json"), 4); // past kept_ttl (3d)
    let seed = only_dir(&only_dir(&w.remote.join("seed")));
    backdate(&seed.join("meta.json"), 8); // past cache_ttl (7d)
    let cache = only_dir(&w.remote.join("cache")); // fresh: kept

    // A running job: its work dir is old but locked.
    let mut busy = w
        .goway(&["run", "--", "sh", "-c", "sleep 4"])
        .spawn()
        .unwrap();
    let mut running = None;
    for _ in 0..100 {
        running = w
            .work_dirs()
            .into_iter()
            .map(|d| w.remote.join("work").join(d))
            .find(|d| *d != kept && d.join("pid").exists());
        if running.is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let running = running.expect("the busy run started");
    backdate(&running.join("meta.json"), 9);
    // The busy run re-touched the seed; age it again.
    backdate(&seed.join("meta.json"), 8);

    let out = w.run(&["gc"]);
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!kept.exists(), "expired kept dir removed\n{stdout}");
    assert!(running.exists(), "locked dir untouched\n{stdout}");
    assert!(stdout.contains("busy"), "{stdout}");
    assert!(cache.exists(), "fresh cache kept");
    assert!(busy.wait().unwrap().success());
    // The seed was locked shared only during the snapshot; it is expired now.
    let again = w.run(&["gc"]);
    assert!(again.status.success());
    assert!(!seed.exists(), "{}", String::from_utf8_lossy(&again.stdout));
}

#[test]
fn gc_dry_run_filters_by_repo_and_age_and_removes_nothing() {
    let w = world();
    assert!(w.run(&["run", "--keep", "--", "true"]).status.success());
    let before = w.work_dirs();
    let other = w.run(&["gc", "--dry-run", "--all", "--repo", "other"]);
    assert!(!String::from_utf8_lossy(&other.stdout).contains("would remove"));
    let young = w.run(&["gc", "--dry-run", "--older-than", "1h", "--repo", "proj"]);
    assert!(!String::from_utf8_lossy(&young.stdout).contains("would remove"));
    let all = w.run(&["gc", "--dry-run", "--all", "--repo", "proj"]);
    let listed = String::from_utf8_lossy(&all.stdout).into_owned();
    assert_eq!(
        listed.matches("would remove").count(),
        3,
        "work, seed, cache\n{listed}"
    );
    assert_eq!(w.work_dirs(), before, "dry run removes nothing");
    assert!(w.remote.join("cache").read_dir().unwrap().next().is_some());
}

#[test]
fn every_run_triggers_automatic_gc_of_expired_entries() {
    let w = world();
    assert!(w.run(&["run", "--keep", "--", "true"]).status.success());
    let kept = w.remote.join("work").join(&w.work_dirs()[0]);
    backdate(&kept.join("meta.json"), 4);
    assert!(w.run(&["run", "--", "true"]).status.success());
    for _ in 0..50 {
        if !kept.exists() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    panic!("automatic gc did not remove the expired kept dir");
}

// frob:tests crates/goway/src/doctor.rs::doctor
// frob:tests crates/goway/src/doctor.rs::parse_facts
#[test]
fn doctor_reports_every_check_with_status() {
    let w = world();
    let out = w.run(&["doctor"]);
    let table = String::from_utf8_lossy(&out.stdout).into_owned();
    for check in [
        "bash",
        "tar",
        "flock",
        "setsid",
        "cc (linker)",
        "cargo",
        "sshd password login",
    ] {
        assert!(
            table.lines().any(|l| l.starts_with(check)),
            "{check} missing\n{table}"
        );
    }
    assert!(String::from_utf8_lossy(&out.stderr).contains("local at 127.0.0.1"));
    let sudo_without_fix = w.run(&["doctor", "--sudo"]);
    assert_eq!(
        sudo_without_fix.status.code(),
        Some(2),
        "--sudo requires --fix"
    );
}

#[test]
fn user_settings_win_over_goway_defaults() {
    let w = world();
    let show = "echo T=$CARGO_TARGET_DIR W=${RUSTC_WRAPPER-unset} D=${SCCACHE_DIR-unset}";
    // --env values win.
    let out = w.run(&[
        "run",
        "-e",
        "CARGO_TARGET_DIR=/custom/target",
        "-e",
        "RUSTC_WRAPPER=",
        "--",
        "sh",
        "-c",
        show,
    ]);
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(text.contains("T=/custom/target"), "{text}");
    assert!(
        text.contains("W= "),
        "empty RUSTC_WRAPPER disables sccache: {text}"
    );
    assert!(text.contains("D=unset"), "{text}");
    // The remote environment wins too (the fake ssh passes ours through).
    let out = w
        .goway(&["run", "--", "sh", "-c", show])
        .env("RUSTC_WRAPPER", "my-wrapper")
        .env("CARGO_TARGET_DIR", "/fast/disk")
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(text.contains("T=/fast/disk W=my-wrapper"), "{text}");
}

/// Every file under `dir`, relative.
fn files_under(dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                stack.push(p);
            } else {
                out.push(p.strip_prefix(dir).unwrap().display().to_string());
            }
        }
    }
    out
}

#[cfg(target_os = "linux")]
#[test]
fn a_run_leaves_no_files_outside_its_root_and_no_processes() {
    let w = world();
    let home = w.root.join("remote-home");
    std::fs::create_dir(&home).unwrap();
    std::fs::write(
        w.config.join("config.toml"),
        "[defaults]\nremote_root = \".cache/goway\"\n\n[[host]]\nname = \"local\"\naddress = \"127.0.0.1\"\n",
    )
    .unwrap();
    let marker = format!("goway-leak-check-{}", std::process::id());
    let out = w
        .goway(&[
            "run",
            "--",
            "sh",
            "-c",
            &format!("echo {marker} >/dev/null; sleep 0.2"),
        ])
        .env("HOME", &home)
        .env_remove("RUSTC_WRAPPER")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let outside: Vec<String> = files_under(&home)
        .into_iter()
        .filter(|f| !f.starts_with(".cache/goway/"))
        .collect();
    assert!(
        outside.is_empty(),
        "files outside the remote root: {outside:?}"
    );
    // Give the detached gc a moment, then no process may mention this run.
    std::thread::sleep(std::time::Duration::from_millis(1500));
    let root = home.join(".cache/goway").display().to_string();
    let mut leftovers = Vec::new();
    for entry in std::fs::read_dir("/proc").unwrap().flatten() {
        let Ok(cmdline) = std::fs::read(entry.path().join("cmdline")) else {
            continue;
        };
        let cmdline = String::from_utf8_lossy(&cmdline).replace('\0', " ");
        if cmdline.contains(&marker) || cmdline.contains(&root) {
            leftovers.push(cmdline);
        }
    }
    assert!(leftovers.is_empty(), "processes left behind: {leftovers:?}");
}

#[cfg(target_os = "linux")]
#[test]
fn low_priority_jobs_run_niced_with_idle_io() {
    let w = world();
    let base: i32 = String::from_utf8_lossy(&Command::new("nice").output().unwrap().stdout)
        .trim()
        .parse()
        .unwrap();
    let show = ["run", "--", "sh", "-c", "nice; ionice"];
    let low = String::from_utf8_lossy(&w.run(&show).stdout).into_owned();
    let mut lines = low.lines();
    assert_eq!(
        lines.next().unwrap().parse::<i32>().unwrap(),
        (base + 10).min(19),
        "{low}"
    );
    assert_eq!(lines.next().unwrap().trim(), "idle", "{low}");

    let mut config = std::fs::read_to_string(w.config.join("config.toml")).unwrap();
    config = config.replace(
        "target_slots = 2",
        "target_slots = 2\npriority = \"normal\"",
    );
    std::fs::write(w.config.join("config.toml"), config).unwrap();
    let normal = String::from_utf8_lossy(&w.run(&show).stdout).into_owned();
    assert_eq!(
        normal.lines().next().unwrap().parse::<i32>().unwrap(),
        base,
        "{normal}"
    );
}

/// Two configured hosts that are both the fake local one.
fn two_hosts() -> common::World {
    let w = world();
    let mut config = std::fs::read_to_string(w.config.join("config.toml")).unwrap();
    config = config.replace("name = \"local\"", "name = \"alpha\"");
    config.push_str("\n[[host]]\nname = \"beta\"\naddress = \"127.0.0.1\"\n");
    std::fs::write(w.config.join("config.toml"), config).unwrap();
    w
}

// frob:tests crates/goway/src/shard.rs::run_sharded
// frob:tests crates/goway/src/pool.rs::choose_many
// frob:tests crates/goway/src/render.rs::prefixed_line
#[test]
fn shards_run_on_n_hosts_and_any_failure_fails_the_run() {
    let w = two_hosts();
    let report = w.root.join("shards.json");
    let script = "echo shard=$GOWAY_SHARD/$GOWAY_SHARD_COUNT; [ \"$GOWAY_SHARD\" != 2 ]";
    let out = w.run(&[
        "run",
        "--shard",
        "2",
        "--report",
        report.to_str().unwrap(),
        "--",
        "sh",
        "-c",
        script,
    ]);
    assert_eq!(
        out.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let mut lines: Vec<&str> = stdout.lines().collect();
    lines.sort_unstable();
    assert_eq!(lines.len(), 2, "{stdout}");
    assert!(lines.iter().any(|l| l.ends_with("shard=1/2")), "{stdout}");
    assert!(lines.iter().any(|l| l.ends_with("shard=2/2")), "{stdout}");
    assert!(
        lines
            .iter()
            .all(|l| l.starts_with("[alpha] ") || l.starts_with("[beta ] ")),
        "{stdout}"
    );
    let json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&report).unwrap()).unwrap();
    assert_eq!(json["exit_code"], 1);
    let hosts: Vec<&str> = json["shards"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["host"].as_str().unwrap())
        .collect();
    assert_eq!(hosts.len(), 2);
    assert_ne!(hosts[0], hosts[1], "each shard on its own host");

    let ok = w.run(&["run", "--shard", "2", "--", "true"]);
    assert_eq!(ok.status.code(), Some(0));
    let too_many = w.run(&["run", "--shard", "3", "--", "true"]);
    assert_eq!(too_many.status.code(), Some(125));
    assert!(String::from_utf8_lossy(&too_many.stderr).contains("3 shards need 3 usable hosts"));
}

#[test]
fn paths_baked_by_a_build_stay_valid_for_later_runs_in_the_slot() {
    let w = world();
    // The "build" bakes its source path into the target dir, as cargo does
    // with CARGO_MANIFEST_DIR; a later run reusing the slot without
    // rebuilding must find the current tree at that path.
    let script = "mkdir -p \"$CARGO_TARGET_DIR\"; \
        if [ -f \"$CARGO_TARGET_DIR/baked\" ]; then cat \"$(cat \"$CARGO_TARGET_DIR/baked\")/hello.txt\"; \
        else pwd > \"$CARGO_TARGET_DIR/baked\"; echo built; fi";
    let first = w.run(&["run", "--", "sh", "-c", script]);
    assert_eq!(String::from_utf8_lossy(&first.stdout), "built\n");
    std::fs::write(w.repo.join("hello.txt"), "changed\n").unwrap();
    let second = w.run(&["run", "--", "sh", "-c", script]);
    assert_eq!(
        String::from_utf8_lossy(&second.stdout),
        "changed\n",
        "{}",
        String::from_utf8_lossy(&second.stderr)
    );
    assert!(w.work_dirs().is_empty(), "work dirs still removed");
    let kept = w.run(&["run", "--keep", "--", "true"]);
    assert!(kept.status.success());
    let dir = w.remote.join("work").join(&w.work_dirs()[0]);
    assert!(
        dir.join("tree/hello.txt").is_file(),
        "--keep keeps the tree in the work dir"
    );
}

#[test]
fn in_place_writes_by_a_job_never_reach_the_seed() {
    let w = world();
    let out = w.run(&["run", "--", "sh", "-c", "echo INJECTED >> hello.txt"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let seeds = w.remote.join("seed");
    let repo_dir = std::fs::read_dir(&seeds)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let seed = std::fs::read_dir(repo_dir)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    assert_eq!(
        std::fs::read_to_string(seed.join("tree/hello.txt")).unwrap(),
        "hello\n",
        "the job's in-place append leaked into the seed"
    );
}

#[test]
fn gc_never_removes_unlabelled_entries_or_anything_under_an_unmarked_root() {
    let w = world();
    assert!(w.run(&["run", "--", "true"]).status.success());
    // An old directory goway did not create, inside goway's root.
    let stray = w.remote.join("work/not-goways");
    std::fs::create_dir_all(&stray).unwrap();
    std::fs::write(stray.join("precious"), "x").unwrap();
    backdate(&stray, 30);
    let out = w.run(&["gc", "--all"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(stray.join("precious").exists(), "unlabelled entry removed");

    // Without the root marker, gc removes nothing at all.
    std::fs::remove_file(w.remote.join(".goway-root")).unwrap();
    assert!(w.run(&["run", "--keep", "--", "true"]).status.success());
    std::fs::remove_file(w.remote.join(".goway-root")).unwrap();
    let kept = w.work_dirs().len();
    let out = w.run(&["gc", "--all"]);
    assert!(out.status.success());
    assert_eq!(w.work_dirs().len(), kept, "gc touched an unmarked root");
}

#[test]
fn local_environment_never_travels_to_the_host() {
    let w = world();
    let out = w
        .goway(&["run", "--", "sh", "-c", "env"])
        .env("MY_API_TOKEN", "s3cret-sentinel")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let remote_env = String::from_utf8_lossy(&out.stdout);
    assert!(!remote_env.contains("s3cret-sentinel"), "{remote_env}");
}

// frob:tests crates/goway/src/run.rs::send_env
#[test]
fn env_values_never_appear_in_ssh_arguments_or_logs() {
    // A fake ssh that records every argument it receives.
    let w = common::world_with_ssh(&FAKE_SSH.replace(
        "while [ $# -gt 0 ]; do",
        "printf '%s\\n' \"$@\" >>\"$HOME/../ssh-argv.log\"\nwhile [ $# -gt 0 ]; do",
    ));
    let home = w.root.join("home");
    std::fs::create_dir(&home).unwrap();
    let out = w
        .goway(&[
            "-vv",
            "run",
            "-e",
            "API_TOKEN=s3cret-sentinel",
            "--",
            "sh",
            "-c",
            "echo got=$API_TOKEN",
        ])
        .env("HOME", &home)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "got=s3cret-sentinel\n",
        "the job still sees it"
    );
    let argv = std::fs::read_to_string(w.root.join("ssh-argv.log")).unwrap();
    assert!(!argv.contains("s3cret-sentinel"), "value in ssh argv");
    let encoded = {
        use base64::Engine as _;
        base64::engine::general_purpose::STANDARD.encode(b"API_TOKEN=s3cret-sentinel\0")
    };
    assert!(!argv.contains(&encoded[..16]), "value (base64) in ssh argv");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("s3cret-sentinel"),
        "value in goway's log: {stderr}"
    );
}

#[test]
fn sccache_listens_on_an_owner_only_socket_not_a_tcp_port() {
    let has_sccache = Command::new("sh")
        .args(["-c", "command -v sccache"])
        .status()
        .is_ok_and(|s| s.success());
    if !has_sccache {
        return;
    }
    let w = world();
    let out = w.run(&[
        "run",
        "--",
        "sh",
        "-c",
        "echo uds=$SCCACHE_SERVER_UDS port=${SCCACHE_SERVER_PORT:-none}",
    ]);
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(text.contains("port=none"), "{text}");
    assert!(
        text.contains("/cache/") && text.contains("sccache.sock"),
        "{text}"
    );
}

#[test]
fn outside_a_git_project_the_error_says_what_to_do() {
    let w = world();
    let elsewhere = w.root.join("not-a-project");
    std::fs::create_dir(&elsewhere).unwrap();
    let out = w
        .goway(&["run", "--", "true"])
        .current_dir(&elsewhere)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(125));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("not in one") && stderr.contains("next: `cd`"),
        "{stderr}"
    );
}

#[test]
fn a_failing_command_is_labelled_failed_in_words() {
    let w = world();
    let out = w.run(&["run", "--", "sh", "-c", "exit 4"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("goway: failed: exit 4 on local"),
        "{stderr}"
    );
}
