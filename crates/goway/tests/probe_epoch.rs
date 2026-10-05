//! The probe and doctor verbs report the host's wall clock as `epoch=SECONDS`.
#![cfg(unix)]

use std::process::Command;

fn epoch_of(verb: &str) -> u64 {
    let home = tempfile::tempdir().unwrap();
    let out = Command::new("bash")
        .args([goway::remote::SCRIPT_SH_PATH, verb])
        .arg(home.path().join("root"))
        .env("HOME", home.path())
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .find_map(|l| l.strip_prefix("epoch="))
        .unwrap_or_else(|| panic!("no epoch= in {verb} output"))
        .parse()
        .unwrap()
}

// frob:tests crates/goway/src/pool.rs::parse_probe
#[test]
fn probe_and_doctor_report_the_host_epoch() {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    for verb in ["probe", "doctor"] {
        let e = epoch_of(verb);
        assert!(e.abs_diff(now) <= 30, "{verb}: epoch {e} vs {now}");
    }
}
