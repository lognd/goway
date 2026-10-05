//! A job cannot take a helper down: its scope carries a process cap, a low CPU weight and
//! optional CPU and memory caps (configured per host), and without systemd `ulimit -u`
//! applies instead.
#![cfg(unix)]

mod common;

use std::os::unix::fs::PermissionsExt;

use goway::config::Config;

/// Add `defaults_lines` under `[defaults]` and `host_lines` under the world's one host.
fn configure(w: &common::World, defaults_lines: &str, host_lines: &str) {
    let path = w.config.join("config.toml");
    let text = std::fs::read_to_string(&path).unwrap();
    let text = text
        .replacen(
            "[defaults]\n",
            &format!("[defaults]\n{defaults_lines}\n"),
            1,
        )
        .replacen(
            "name = \"local\"\n",
            &format!("name = \"local\"\n{host_lines}\n"),
            1,
        );
    std::fs::write(path, text).unwrap();
}

/// A `systemd-run` that records its arguments in `log` and runs the command like systemd
/// does (options dropped), or fails when `works` is false (a helper with no user manager).
fn fake_systemd_run(w: &common::World, log: &std::path::Path, works: bool) {
    let body = if works {
        format!(
            "#!/bin/bash\necho \"$*\" >> '{}'\nwhile [ \"${{1#--}}\" != \"$1\" ]; do shift; done\nexec \"$@\"\n",
            log.display()
        )
    } else {
        "#!/bin/sh\nexit 1\n".to_owned()
    };
    let fake = w.bin.join("systemd-run");
    std::fs::write(&fake, body).unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
}

// frob:ticket 01M44F0HBMG5K7VA1SSHNJDP23
// frob:tests crates/goway/src/config.rs::limits_word
#[test]
fn limits_default_to_a_process_cap_and_hosts_override_each_one() {
    let config: Config = toml::from_str(
        "[defaults]\njob_cpu = \"800%\"\n\n[[host]]\nname = \"helios\"\njob_tasks = 100\njob_memory = \"6G\"\n\n[[host]]\nname = \"orion\"\n",
    )
    .unwrap();
    let helios = config.host("helios").unwrap();
    let orion = config.host("orion").unwrap();
    assert_eq!(
        config.for_host(helios).defaults.limits_word(),
        format!("limits:100:800:{}", 6u64 << 30)
    );
    assert_eq!(
        config.for_host(orion).defaults.limits_word(),
        "limits:auto:800:"
    );
    assert_eq!(Config::default().defaults.limits_word(), "limits:auto::");
}

// frob:ticket 01M44F0HBMG5K7VA1SSHNJDP23
// frob:tests crates/goway/src/config.rs::limits_word
#[test]
fn a_cpu_cap_that_is_not_a_percentage_is_refused() {
    let w = common::world();
    configure(&w, "job_cpu = \"fast\"", "");
    let out = w.run(&["run", "--", "true"]);
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("job_cpu"),
        "{out:?}"
    );
}

// frob:ticket 01M44F0HBMG5K7VA1SSHNJDP23
// frob:tests crates/goway/src/run.rs::run_invocation_with
// Linux only: a transient systemd scope exists nowhere else (macOS has no systemd, and
// remote.sh skips the scope there by design), so there is no scope to inspect.
#[cfg(target_os = "linux")]
#[test]
fn a_job_scope_carries_the_process_cap_a_low_cpu_weight_and_the_configured_caps() {
    // remote.sh puts a job in a scope only under cgroup v2 (a cgroup v1 or hybrid machine,
    // such as an older WSL, runs it with the `ulimit -u` fallback instead).
    if !std::path::Path::new("/sys/fs/cgroup/cgroup.controllers").exists() {
        #[allow(
            clippy::print_stderr,
            reason = "a skipped test says why on the test's own output"
        )]
        {
            eprintln!("skipped: no cgroup v2 here, so no job scope exists to inspect");
        }
        return;
    }
    let w = common::world();
    let log = w.root.join("systemd-run.log");
    fake_systemd_run(&w, &log, true);
    configure(
        &w,
        "job_cpu = \"800%\"\njob_tasks = 4096",
        "job_memory = \"2G\"",
    );
    let out = w.run(&["run", "--", "true"]);
    assert!(out.status.success(), "{out:?}");
    let log = std::fs::read_to_string(&log).unwrap();
    for want in [
        "--property=TasksMax=4096",
        "--property=CPUWeight=20",
        "--property=CPUQuota=800%",
        &format!("--property=MemoryMax={}", 2u64 << 30),
        "--unit=goway-",
    ] {
        assert!(log.contains(want), "{want} missing from {log}");
    }
}

// frob:ticket 01M44F0HBMG5K7VA1SSHNJDP23
// frob:tests crates/goway/src/run.rs::run_invocation_with
#[test]
fn without_a_user_manager_the_process_cap_is_a_ulimit_and_zero_lifts_it() {
    let w = common::world();
    fake_systemd_run(&w, &w.root.join("unused.log"), false);
    configure(&w, "job_tasks = 6000", "");
    let out = w.run(&["run", "--", "bash", "-c", "ulimit -u"]);
    assert!(out.status.success(), "{out:?}");
    // The user's own processes at start come on top of the job's 6000, but a cap is never
    // looser than the limit the job inherits (1333 on a macOS runner): then it stays there.
    let soft = std::process::Command::new("bash")
        .args(["-c", "ulimit -u"])
        .output()
        .unwrap();
    let soft = String::from_utf8_lossy(&soft.stdout)
        .trim()
        .parse()
        .unwrap_or(u64::MAX);
    let cap: u64 = String::from_utf8_lossy(&out.stdout).trim().parse().unwrap();
    assert!(
        cap >= 6000.min(soft),
        "{out:?} with an inherited limit of {soft}"
    );

    let w = common::world();
    fake_systemd_run(&w, &w.root.join("unused.log"), false);
    configure(&w, "job_tasks = 6000", "job_tasks = 0");
    let unlimited = std::process::Command::new("bash")
        .args(["-c", "ulimit -u"])
        .output()
        .unwrap();
    let out = w.run(&["run", "--", "bash", "-c", "ulimit -u"]);
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        String::from_utf8_lossy(&unlimited.stdout).trim(),
        "{out:?} against the unrestricted {unlimited:?}"
    );
}

/// Run `true` under a fake systemd-run with `threads-max` faked as `max` (and no cgroup
/// bound), returning the `TasksMax` the scope was given, or `None` when no scope exists.
#[cfg(target_os = "linux")]
fn derived_tasks_max(max: &str) -> Option<String> {
    let w = common::world();
    let log = w.root.join("systemd-run.log");
    fake_systemd_run(&w, &log, true);
    let proc = w.root.join("fake-proc-kernel");
    std::fs::create_dir_all(&proc).unwrap();
    std::fs::write(proc.join("threads-max"), format!("{max}\n")).unwrap();
    let out = w
        .goway(&["run", "--", "true"])
        .env("GOWAY_TASKS_PROC", &proc)
        .env("GOWAY_TASKS_CGROUP", w.root.join("no-cgroup"))
        .env(
            "GOWAY_SSH_PASS_ENV",
            "FAKE_HOSTNAME,FAKE_WINDOWS_PORT,RUSTC_WRAPPER,CARGO_TARGET_DIR,GOWAY_WINDOWS_LOOKUP,GOWAY_WSL_PROC,GOWAY_TASKS_PROC,GOWAY_TASKS_CGROUP",
        )
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    let log = std::fs::read_to_string(&log).ok()?;
    log.split_whitespace()
        .find_map(|a| a.strip_prefix("--property=TasksMax="))
        .map(str::to_owned)
}

// frob:ticket 01M451D50SW5GSZDGSMZQ447N2
// frob:tests crates/goway/src/config.rs::limits_word
// Linux with cgroup v2 only, like the scope test above. The host's own hard process limit
// also bounds the cap, so the expected values are clamped to it.
#[cfg(target_os = "linux")]
#[test]
fn the_default_task_cap_is_half_of_the_hosts_threads_max_with_a_floor() {
    if !std::path::Path::new("/sys/fs/cgroup/cgroup.controllers").exists() {
        return;
    }
    let hard = std::process::Command::new("bash")
        .args(["-c", "ulimit -H -u"])
        .output()
        .unwrap();
    let hard: u64 = String::from_utf8_lossy(&hard.stdout)
        .trim()
        .parse()
        .unwrap_or(u64::MAX);
    let expect = |max: u64| {
        let bound = max.min(hard);
        (max / 2).min(hard).max(16384.min(bound)).to_string()
    };
    assert_eq!(derived_tasks_max("62570"), Some(expect(62570)));
    assert_eq!(derived_tasks_max("1000000"), Some(expect(1_000_000)));
    // Below the floor the cap follows the bound, never above it.
    assert_eq!(derived_tasks_max("8000"), Some(expect(8000)));
}
