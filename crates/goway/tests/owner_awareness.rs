//! A helper whose owner is using it (on battery, or recently at the keyboard)
//! stays available, but jobs there run extra nicely; an unreadable state
//! changes nothing. Through the fake-ssh world, with a fake power-supply
//! directory standing in for the kernel's.
#![cfg(unix)]

mod common;

use std::path::{Path, PathBuf};

/// A power-supply directory: a battery with `status` and an adapter that is `online`.
fn supplies(w: &common::World, status: &str, online: bool) -> PathBuf {
    let dir = w.root.join("power_supply");
    for (name, kind) in [("BAT0", "Battery"), ("AC0", "Mains")] {
        let d = dir.join(name);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("type"), format!("{kind}\n")).unwrap();
    }
    std::fs::write(dir.join("BAT0/status"), format!("{status}\n")).unwrap();
    std::fs::write(dir.join("AC0/online"), if online { "1\n" } else { "0\n" }).unwrap();
    dir
}

/// The `goway` command with the fake power-supply directory passed through the fake ssh.
fn with_power(w: &common::World, args: &[&str], power: &Path) -> std::process::Command {
    // A Mac's idle time comes from ioreg: unless a test says otherwise it is unknown.
    if !w.bin.join("ioreg").exists() {
        fake_ioreg(w, None);
    }
    let mut cmd = w.goway(args);
    cmd.env("GOWAY_POWER_SUPPLY_DIR", power).env(
        "GOWAY_SSH_PASS_ENV",
        "FAKE_HOSTNAME,FAKE_WINDOWS_PORT,RUSTC_WRAPPER,CARGO_TARGET_DIR,GOWAY_WINDOWS_LOOKUP,GOWAY_POWER_SUPPLY_DIR",
    );
    cmd
}

/// Make the world's `ioreg` report a HID idle time of `secs` (none: no answer).
fn fake_ioreg(w: &common::World, secs: Option<u64>) {
    use std::os::unix::fs::PermissionsExt as _;
    let body = secs.map_or_else(String::new, |s| {
        format!("echo '    | \"HIDIdleTime\" = {}'\n", s * 1_000_000_000)
    });
    let path = w.bin.join("ioreg");
    std::fs::write(&path, format!("#!/bin/sh\n{body}")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn run(w: &common::World, power: &Path, args: &[&str], script: &str) -> std::process::Output {
    let mut all = vec!["run"];
    all.extend_from_slice(args);
    all.extend(["--", "sh", "-c", script]);
    with_power(w, &all, power).output().unwrap()
}

fn out(o: &std::process::Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn err(o: &std::process::Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

const SHOW: &str = "nice; echo \"jobs=${CARGO_BUILD_JOBS-unset} make=${MAKEFLAGS-unset} cmake=${CMAKE_BUILD_PARALLEL_LEVEL-unset} nextest=${NEXTEST_TEST_THREADS-unset}\"";

/// This process's own niceness (the suite may run niced, or with a negative one on a CI runner).
fn base_nice() -> i32 {
    let n = std::process::Command::new("nice").output().unwrap();
    String::from_utf8_lossy(&n.stdout).trim().parse().unwrap()
}

/// The niceness a job gets at the default priority: this process's own plus 10, at most 19.
fn low_nice() -> i32 {
    (base_nice() + 10).min(19)
}

/// The niceness of a job on a helper in use: this process's own plus 19, at most 19.
fn owner_nice() -> i32 {
    (base_nice() + 19).min(19)
}

fn half_the_cores() -> u32 {
    let n = std::process::Command::new("nproc").output().unwrap();
    let cores: u32 = String::from_utf8_lossy(&n.stdout).trim().parse().unwrap();
    (cores / 2).max(1)
}

// frob:tests crates/goway/src/pool.rs::priority_word
#[test]
fn a_helper_on_battery_is_still_used_but_extra_nicely_with_half_the_cores() {
    let w = common::world();
    let dir = supplies(&w, "Discharging", false);
    let o = run(&w, &dir, &[], SHOW);
    assert_eq!(o.status.code(), Some(0), "never skipped: {}", err(&o));
    let half = half_the_cores();
    let text = out(&o);
    assert!(
        text.starts_with(&format!("{}\n", owner_nice())),
        "nice 19: {text}"
    );
    assert!(
        text.contains(&format!(
            "jobs={half} make=-j{half} cmake={half} nextest={half}"
        )),
        "{text}"
    );
    assert!(
        err(&o).contains("extra nicely"),
        "one line says so: {}",
        err(&o)
    );
    assert_eq!(err(&o).matches("extra nicely").count(), 1);
}

// frob:tests crates/goway/src/pool.rs::priority_word
#[test]
fn a_helper_on_mains_runs_at_the_normal_polite_priority_with_no_caps() {
    let w = common::world();
    let dir = supplies(&w, "Charging", true);
    let o = run(&w, &dir, &[], SHOW);
    assert_eq!(o.status.code(), Some(0), "{}", err(&o));
    let text = out(&o);
    assert!(
        text.starts_with(&format!("{}\n", low_nice())),
        "nice 10: {text}"
    );
    assert!(
        text.contains("jobs=unset make=unset cmake=unset nextest=unset"),
        "{text}"
    );
    assert!(!err(&o).contains("extra nicely"), "{}", err(&o));
}

// frob:tests crates/goway/src/pool.rs::owner_use
#[test]
fn an_unreadable_state_is_unknown_and_changes_nothing() {
    let w = common::world();
    let o = run(&w, &w.root.join("no-such-dir"), &[], SHOW);
    assert_eq!(o.status.code(), Some(0), "{}", err(&o));
    assert!(
        out(&o).starts_with(&format!("{}\n", low_nice())),
        "{}",
        out(&o)
    );
    assert!(!err(&o).contains("extra nicely"), "{}", err(&o));
    let status = with_power(&w, &["status"], &w.root.join("no-such-dir"))
        .output()
        .unwrap();
    assert!(out(&status).contains("owner"), "{}", out(&status));
}

// frob:tests crates/goway/src/pool.rs::owner_note
#[test]
fn settings_the_user_made_stay_theirs() {
    let w = common::world();
    let dir = supplies(&w, "Discharging", false);
    let o = run(
        &w,
        &dir,
        &["--env", "CARGO_BUILD_JOBS=7", "--env", "MAKEFLAGS=-j3"],
        SHOW,
    );
    assert_eq!(o.status.code(), Some(0), "{}", err(&o));
    let half = half_the_cores();
    assert!(
        out(&o).contains(&format!("jobs=7 make=-j3 cmake={half} nextest={half}")),
        "{}",
        out(&o)
    );
}

// frob:tests crates/goway/src/status.rs::rows
#[test]
fn status_shows_the_owner_state() {
    let w = common::world();
    let dir = supplies(&w, "Discharging", false);
    let o = with_power(&w, &["status"], &dir).output().unwrap();
    assert!(out(&o).contains("on battery"), "{}{}", out(&o), err(&o));
}

// frob:tests crates/goway/src/config.rs::Defaults
#[test]
fn a_zero_idle_window_switches_the_awareness_off() {
    let w = common::world();
    let cfg = std::fs::read_to_string(w.config.join("config.toml")).unwrap();
    std::fs::write(
        w.config.join("config.toml"),
        cfg.replace("target_slots = 2", "target_slots = 2\nowner_idle = \"0s\""),
    )
    .unwrap();
    let dir = supplies(&w, "Discharging", false);
    let o = run(&w, &dir, &[], SHOW);
    assert_eq!(o.status.code(), Some(0), "{}", err(&o));
    assert!(
        out(&o).starts_with(&format!("{}\n", low_nice())),
        "{}",
        out(&o)
    );
}

/// Make the world's `powershell.exe` answer the idle-time query with `secs`.
fn windows_idle(w: &common::World, secs: u64) {
    use std::os::unix::fs::PermissionsExt as _;
    let path = w.bin.join("powershell.exe");
    std::fs::write(&path, format!("#!/bin/sh\necho {secs}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    fake_ioreg(w, Some(secs));
}

// frob:tests crates/goway/src/pool.rs::owner_use
#[test]
fn a_user_at_the_keyboard_within_the_window_counts_as_in_use_and_a_long_idle_one_does_not() {
    // The script asks Windows on WSL and ioreg on a Mac; elsewhere the idle time is simply unknown.
    let wsl = std::fs::read_to_string("/proc/version")
        .is_ok_and(|v| v.to_lowercase().contains("microsoft"));
    if !wsl && !cfg!(target_os = "macos") {
        return;
    }
    let w = common::world();
    let none = w.root.join("no-such-dir");
    windows_idle(&w, 42);
    let o = run(&w, &none, &[], SHOW);
    assert!(
        out(&o).starts_with(&format!("{}\n", owner_nice())),
        "{}{}",
        out(&o),
        err(&o)
    );
    assert!(err(&o).contains("extra nicely"), "{}", err(&o));
    windows_idle(&w, 9000);
    let o = run(&w, &none, &[], SHOW);
    assert!(
        out(&o).starts_with(&format!("{}\n", low_nice())),
        "{}{}",
        out(&o),
        err(&o)
    );
}
