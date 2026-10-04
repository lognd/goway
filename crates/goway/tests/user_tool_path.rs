//! Per-user tool directories reach non-interactive runs without sourcing any
//! startup file, and doctor names proxy variables without their values.
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::process::Command;

const SCRIPT: &str = include_str!("../src/remote.sh");

fn doctor(home: &std::path::Path, tool: &str) -> String {
    let out = Command::new("bash")
        .args(["-c", SCRIPT, "goway", "doctor", "state", tool])
        .env("HOME", home)
        .env("HTTPS_PROXY", "http://user:hunter2@proxy.example:3128")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn fake_tool(dir: &std::path::Path, name: &str) {
    std::fs::create_dir_all(dir).unwrap();
    let p = dir.join(name);
    std::fs::write(&p, "#!/bin/sh\necho 1.0\n").unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn user_level_tool_directories_are_found_without_startup_files() {
    let home = tempfile::tempdir().unwrap();
    fake_tool(&home.path().join(".local/bin"), "gowaymold");
    fake_tool(&home.path().join(".cargo/bin"), "gowaycargotool");
    // A startup file that would break or print must never be read.
    std::fs::write(home.path().join(".profile"), "echo noise; exit 3\n").unwrap();
    std::fs::write(home.path().join(".bashrc"), "echo noise; exit 3\n").unwrap();
    let out = doctor(home.path(), "gowaymold");
    assert!(out.contains("want.gowaymold=1.0"), "{out}");
    let out = doctor(home.path(), "gowaycargotool");
    assert!(out.contains("want.gowaycargotool=1.0"), "{out}");
}

#[test]
fn doctor_reports_proxy_variable_names_never_values() {
    let home = tempfile::tempdir().unwrap();
    let out = doctor(home.path(), "git");
    assert!(out.contains("proxy_vars=HTTPS_PROXY"), "{out}");
    assert!(
        !out.contains("hunter2") && !out.contains("proxy.example"),
        "{out}"
    );
}

#[test]
fn runs_and_doctor_share_one_path_function() {
    assert_eq!(
        SCRIPT.matches("\n  user_tool_path\n").count(),
        2,
        "both the run and the doctor verbs call user_tool_path"
    );
    assert!(!SCRIPT.contains(". \"$HOME/.cargo/env\""), "no sourcing");
}
