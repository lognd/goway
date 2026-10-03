//! This machine as a place to run: `--host local`, `[local]` pool and
//! fallback, and the status row, through the fake-ssh world.
#![cfg(unix)]

mod common;

/// A world whose only helper is `alpha` (fake ssh to this machine) plus `extra` config.
fn world_with(extra: &str) -> common::World {
    configure(common::world(), extra)
}

/// A world whose only helper cannot be reached.
fn offline_world(extra: &str) -> common::World {
    configure(common::world_with_ssh("#!/bin/sh\nexit 255\n"), extra)
}

fn configure(w: common::World, extra: &str) -> common::World {
    let config = format!(
        "[defaults]\nremote_root = \"{}\"\ntarget_slots = 2\n\n[[host]]\nname = \"alpha\"\naddress = \"127.0.0.1\"\n\n{extra}\n",
        w.remote.display()
    );
    std::fs::write(w.config.join("config.toml"), config).unwrap();
    w
}

fn err(out: &std::process::Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn out(out: &std::process::Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// `goway run [flags before --] --report P -- cmd`; returns the output and the report.
fn report(w: &common::World, args: &[&str]) -> (std::process::Output, serde_json::Value) {
    let path = w.root.join("report.json");
    let _ = std::fs::remove_file(&path);
    let split = args.iter().position(|a| *a == "--").unwrap();
    let mut cmd = vec!["run"];
    cmd.extend_from_slice(&args[..split]);
    cmd.extend(["--report", path.to_str().unwrap()]);
    cmd.extend_from_slice(&args[split..]);
    let o = w.run(&cmd);
    let json = std::fs::read_to_string(&path).map_or(serde_json::Value::Null, |t| {
        serde_json::from_str(&t).unwrap()
    });
    (o, json)
}

// frob:tests crates/goway/src/local.rs::run_here
#[test]
fn host_local_runs_in_place_without_sync_and_keeps_the_exit_code() {
    let w = world_with("");
    std::fs::write(w.repo.join("marker.txt"), "untracked and unsynced\n").unwrap();
    std::fs::create_dir_all(w.repo.join("sub")).unwrap();
    let (o, r) = report(
        &w,
        &[
            "--host",
            "local",
            "--",
            "sh",
            "-c",
            "pwd; cat marker.txt 2>/dev/null; echo oops >&2; exit 3",
        ],
    );
    assert_eq!(o.status.code(), Some(3), "{}", err(&o));
    let stdout = out(&o);
    let cwd = std::fs::canonicalize(&w.repo).unwrap();
    assert!(
        stdout.starts_with(&format!("{}\n", cwd.display())),
        "{stdout}"
    );
    assert!(stdout.contains("untracked and unsynced"), "{stdout}");
    assert!(
        err(&o).contains("oops") && err(&o).contains("running on local"),
        "{}",
        err(&o)
    );
    assert!(!w.remote.exists(), "nothing was synced anywhere");
    assert_eq!(r["host"], "local");
    assert_eq!(r["arch"], std::env::consts::ARCH);
    assert_eq!(r["exit_code"], 3);
    // From a subdirectory it runs in that directory.
    let o = w
        .goway(&["run", "--host", "local", "--", "pwd"])
        .current_dir(w.repo.join("sub"))
        .output()
        .unwrap();
    assert!(out(&o).trim_end().ends_with("/sub"), "{}", out(&o));
    // A command that does not exist is a shell-style 127.
    let o = w.run(&["run", "--host", "local", "--", "no-such-program-here"]);
    assert_eq!(o.status.code(), Some(127), "{}", err(&o));
}

// frob:tests crates/goway/src/local.rs::wrapped
#[test]
fn local_runs_use_the_configured_priority() {
    let niceness = |extra: &str| -> i32 {
        let w = world_with(extra);
        let o = w.run(&["run", "--host", "local", "--", "nice"]);
        out(&o).trim().parse().unwrap()
    };
    let base: i32 =
        String::from_utf8_lossy(&std::process::Command::new("nice").output().unwrap().stdout)
            .trim()
            .parse()
            .unwrap();
    // `defaults.priority` is low, and `[local]` inherits it.
    assert_eq!(niceness(""), (base + 10).min(19));
    assert_eq!(niceness("[local]\npriority = \"normal\""), base);
}

// frob:tests crates/goway/src/pool.rs::choose
#[test]
fn local_is_never_chosen_without_the_opt_in() {
    for extra in ["", "[local]\npool = false\nfallback = false"] {
        let w = world_with(extra);
        for _ in 0..2 {
            let (o, r) = report(&w, &["--", "true"]);
            assert!(o.status.success(), "{}", err(&o));
            assert_eq!(r["host"], "alpha");
        }
        // Two shards need two hosts; this machine does not count.
        let o = w.run(&["run", "--shard", "2", "--", "true"]);
        assert_eq!(o.status.code(), Some(125));
        assert!(
            err(&o).contains("2 shards need 2 usable hosts"),
            "{}",
            err(&o)
        );
    }
}

// frob:tests crates/goway/src/pool.rs::choose
#[test]
fn offline_means_failure_that_offers_local_unless_fallback_is_set() {
    let w = offline_world("");
    let o = w.run(&["run", "--", "echo", "hi"]);
    assert_eq!(o.status.code(), Some(125));
    let e = err(&o);
    assert!(e.contains("alpha:"), "the reason is listed: {e}");
    assert!(e.contains("--host local"), "{e}");
    assert!(e.contains("[local] fallback = true"), "{e}");
    // The pool opt-in alone is not the fallback ... but it does make local a candidate.
    let w = offline_world("[local]\nfallback = true");
    let (o, r) = report(&w, &["--", "sh", "-c", "echo ran-here; exit 4"]);
    assert_eq!(o.status.code(), Some(4), "{}", err(&o));
    assert!(out(&o).contains("ran-here"));
    assert!(
        err(&o).contains(
            "no helper is reachable; running on this machine because [local] fallback = true"
        ),
        "{}",
        err(&o)
    );
    assert_eq!(r["host"], "local");
    // With the helper reachable the fallback is not used.
    let w = world_with("[local]\nfallback = true");
    let (o, r) = report(&w, &["--", "true"]);
    assert!(o.status.success(), "{}", err(&o));
    assert_eq!(r["host"], "alpha");
}

// frob:tests crates/goway/src/pool.rs::choose
#[test]
fn a_pooled_local_machine_competes_with_a_margin_and_respects_max_jobs() {
    // Offline helper: the pooled machine is the only candidate.
    let w = offline_world("[local]\npool = true");
    let (o, r) = report(&w, &["--", "true"]);
    assert!(o.status.success(), "{}", err(&o));
    assert_eq!(r["host"], "local");
    // Same load on both: the margin keeps the run on the helper.
    let w = world_with("[local]\npool = true");
    for _ in 0..2 {
        let (o, r) = report(&w, &["--", "true"]);
        assert!(o.status.success(), "{}", err(&o));
        assert_eq!(r["host"], "alpha", "the margin favours the helper");
    }
    // max_jobs = 1: with one local job running, the pooled machine is skipped.
    let w = offline_world("[local]\npool = true");
    // Warm the cached host facts (the first probe of this machine is slow).
    assert!(
        w.run(&["run", "--host", "local", "--", "true"])
            .status
            .success()
    );
    let mut long = w
        .goway(&["run", "--host", "local", "--", "sleep", "6"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    // Wait until it holds its slot.
    let jobs = w.root.join("state/local-jobs");
    let mut running = false;
    for _ in 0..100 {
        running = std::fs::read_dir(&jobs).is_ok_and(|d| d.count() > 0);
        if running {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    assert!(running, "the local job never took its slot");
    let o = w.run(&["run", "--", "true"]);
    assert_eq!(o.status.code(), Some(125), "{}", err(&o));
    assert!(err(&o).contains("local: load"), "{}", err(&o));
    let _ = long.wait();
    let (o, r) = report(&w, &["--", "true"]);
    assert!(o.status.success(), "the slot is free again: {}", err(&o));
    assert_eq!(r["host"], "local");
}

// frob:tests crates/goway/src/pool.rs::choose_many
#[test]
fn a_pooled_local_machine_can_take_a_shard() {
    let w = world_with("[local]\npool = true\nmax_jobs = 2");
    let (o, r) = report(
        &w,
        &[
            "--shard",
            "2",
            "--",
            "sh",
            "-c",
            "echo shard=$GOWAY_SHARD/$GOWAY_SHARD_COUNT in $(pwd)",
        ],
    );
    assert!(o.status.success(), "{}", err(&o));
    let mut hosts: Vec<String> = r["shards"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["host"].as_str().unwrap().to_owned())
        .collect();
    hosts.sort();
    assert_eq!(hosts, ["alpha", "local"]);
    let stdout = out(&o);
    assert!(
        stdout.contains("shard=1/2") && stdout.contains("shard=2/2"),
        "{stdout}"
    );
    let cwd = std::fs::canonicalize(&w.repo).unwrap();
    assert!(
        stdout
            .lines()
            .any(|l| l.starts_with("[local") && l.contains(&format!("in {}", cwd.display()))),
        "the local shard ran in place: {stdout}"
    );
}

// frob:tests crates/goway/src/status.rs::rows
#[test]
fn status_shows_a_local_row_and_whether_it_is_in_the_pool() {
    for (extra, expect) in [
        ("[local]\npool = true", "in the pool"),
        ("[local]", "not in the pool"),
    ] {
        let w = world_with(extra);
        let o = w.run(&["status"]);
        assert!(o.status.success(), "{}", err(&o));
        let table = out(&o);
        let line = table
            .lines()
            .find(|l| l.starts_with("local"))
            .unwrap_or_else(|| panic!("{table}"));
        assert!(line.contains(expect), "{line}");
        assert!(line.contains("this machine"), "{line}");
    }
    let w = world_with("");
    let table = out(&w.run(&["status"]));
    assert!(!table.lines().any(|l| l.starts_with("local")), "{table}");
}
