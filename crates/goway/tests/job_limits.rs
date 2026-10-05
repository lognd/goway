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
        "limits:4096:800:"
    );
    assert_eq!(Config::default().defaults.limits_word(), "limits:4096::");
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
        "{out:?} against the unrestricted {unlimited:?}"
    );
}

// frob:ticket 01M44F0HBMG5K7VA1SSHNJDP23
// frob:tests crates/goway/src/run.rs::run_invocation_with
// Linux only: a transient systemd scope exists nowhere else (macOS has no systemd, and
// remote.sh skips the scope there by design), so there is no scope to inspect.
#[cfg(target_os = "linux")]
#[test]
fn a_job_scope_carries_the_process_cap_a_low_cpu_weight_and_the_configured_caps() {
    let w = common::world();
    let log = w.root.join("systemd-run.log");
    fake_systemd_run(&w, &log, true);
    configure(&w, "job_cpu = \"800%\"", "job_memory = \"2G\"");
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
    // The user's own processes at start come on top of the job's 6000, but `ulimit -u` cannot
    // be raised past the platform's hard limit (1333 on a macOS runner): then it stays there.
    let hard = std::process::Command::new("bash")
        .args(["-c", "ulimit -Hu"])
        .output()
        .unwrap();
    let hard = String::from_utf8_lossy(&hard.stdout)
        .trim()
        .parse()
        .unwrap_or(u64::MAX);
    let cap: u64 = String::from_utf8_lossy(&out.stdout).trim().parse().unwrap();
    assert!(cap >= 6000.min(hard), "{out:?}");

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
