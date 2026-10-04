//! The probe verb reports tool versions when asked with `tools:A,B`.
#![cfg(unix)]

use std::process::Command;

const SCRIPT: &str = include_str!("../src/remote.sh");

// frob:tests crates/goway/src/pool.rs::probe_call
#[test]
fn the_probe_reports_want_lines_only_for_the_tools_it_is_asked_about() {
    let home = tempfile::tempdir().unwrap();
    let run = |extra: &[&str]| {
        let out = Command::new("bash")
            .args(["-c", SCRIPT, "goway", "probe"])
            .arg(home.path().join("root"))
            .args(extra)
            .env("HOME", home.path())
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    let plain = run(&[]);
    assert!(!plain.contains("want."), "{plain}");
    let asked = run(&["tools:bash,goway-no-such-tool,bad;name"]);
    assert!(
        asked
            .lines()
            .any(|l| l.starts_with("want.bash=") && l.len() > "want.bash=".len()),
        "{asked}"
    );
    assert!(asked.contains("want.goway-no-such-tool=\n"), "{asked}");
    assert!(!asked.contains("bad;name"), "{asked}");
    assert!(
        asked.contains("jobs="),
        "the usual facts still come: {asked}"
    );
}
