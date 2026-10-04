//! Repository footprints: each helper records the peak disk a repository
//! used, reports it in its probe, makes room before a run, and explains a
//! failure on a (nearly) full disk.
#![cfg(unix)]

mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::Path;

const MIB: u64 = 1 << 20;
const GIB: u64 = 1 << 30;

/// A `df` that reports `avail` bytes free of `size` for the `--output=` forms goway asks.
fn fake_df(w: &common::World, avail: u64, size: u64) {
    let df = w.bin.join("df");
    std::fs::write(
        &df,
        format!(
            "#!/bin/sh\ncase \"$*\" in\n *--output=avail*) echo Avail; echo {avail} ;;\n *--output=size*) echo Size; echo {size} ;;\n *) exec /usr/bin/env -i PATH=/usr/bin:/bin df \"$@\" ;;\nesac\n"
        ),
    )
    .unwrap();
    std::fs::set_permissions(&df, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn footprint_files(root: &Path) -> Vec<std::path::PathBuf> {
    std::fs::read_dir(root.join("footprints"))
        .map(|d| d.filter_map(|e| e.ok().map(|e| e.path())).collect())
        .unwrap_or_default()
}

fn recorded(root: &Path) -> Option<u64> {
    let f = footprint_files(root).into_iter().next()?;
    std::fs::read_to_string(f).ok()?.trim().parse().ok()
}

fn probe(w: &common::World, extra: &[&str]) -> String {
    let home = w.root.join("probe-home");
    std::fs::create_dir_all(&home).unwrap();
    let out = std::process::Command::new("bash")
        .args(["-c", include_str!("../src/remote.sh"), "goway", "probe"])
        .arg(&w.remote)
        .args(extra)
        .env("HOME", &home)
        .env(
            "PATH",
            format!(
                "{}:{}",
                w.bin.display(),
                std::env::var("PATH").unwrap_or_default()
            ),
        )
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    String::from_utf8(out.stdout).unwrap()
}

// frob:tests crates/goway/src/footprint.rs::parse
#[test]
fn a_run_records_the_peak_footprint_and_the_probe_reports_it() {
    let w = common::world();
    let out = w.run(&[
        "run",
        "--",
        "sh",
        "-c",
        "mkdir -p \"$CARGO_TARGET_DIR\" && truncate -s 3M \"$CARGO_TARGET_DIR/big\"",
    ]);
    assert!(out.status.success(), "{out:?}");
    common::wait_for("the footprint record", || {
        recorded(&w.remote).is_some_and(|b| b >= 3 * MIB)
    });
    let text = probe(&w, &[]);
    let line = text
        .lines()
        .find(|l| l.starts_with("footprint."))
        .unwrap_or_else(|| panic!("{text}"));
    assert!(line.split_once('=').unwrap().1.parse::<u64>().unwrap() >= 3 * MIB);
    assert!(text.contains("disk_free="), "{text}");
    // Plenty of room: the costly du of the whole root is not offered.
    assert!(!text.contains("disk_used="), "{text}");
    // A smaller later run never lowers the peak.
    let peak = recorded(&w.remote).unwrap();
    let out = w.run(&["run", "--", "true"]);
    assert!(out.status.success(), "{out:?}");
    assert!(recorded(&w.remote).unwrap() >= peak);
}

// frob:tests crates/goway/src/footprint.rs::assess
#[test]
fn a_tight_disk_makes_the_probe_report_what_goway_holds() {
    let w = common::world();
    let out = w.run(&[
        "run",
        "--",
        "sh",
        "-c",
        "mkdir -p \"$CARGO_TARGET_DIR\" && truncate -s 3M \"$CARGO_TARGET_DIR/big\"",
    ]);
    assert!(out.status.success(), "{out:?}");
    common::wait_for("the footprint record", || recorded(&w.remote).is_some());
    fake_df(&w, 100 * MIB, 10 * GIB);
    let text = probe(&w, &[]);
    assert!(text.contains("disk_free=104857600"), "{text}");
    assert!(text.contains("disk_used="), "{text}");
}

// frob:tests crates/goway/src/footprint.rs::short_text
#[test]
fn a_failure_on_a_nearly_full_disk_is_explained_in_plain_words() {
    let w = common::world();
    fake_df(&w, 100 * MIB, 10 * GIB);
    let out = w.run(&["run", "--", "sh", "-c", "exit 3"]);
    let err = String::from_utf8_lossy(&out.stderr).into_owned();
    assert_eq!(out.status.code(), Some(3), "{err}");
    assert!(err.contains("this host ran out of disk"), "{err}");
    assert!(err.contains("the disk budget freed"), "{err}");
    assert!(err.contains("--needs disk>="), "{err}");
    // With room, a failure is just the command's own.
    let w2 = common::world();
    let out = w2.run(&["run", "--", "sh", "-c", "exit 3"]);
    let err = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(!err.contains("ran out of disk"), "{err}");
}

// frob:tests crates/goway/src/footprint.rs::required
#[test]
fn a_run_short_of_room_evicts_idle_caches_first_and_says_so() {
    let w = common::world();
    // A first run records a footprint, then an idle cache of another repository exists.
    let out = w.run(&[
        "run",
        "--",
        "sh",
        "-c",
        "mkdir -p \"$CARGO_TARGET_DIR\" && truncate -s 3M \"$CARGO_TARGET_DIR/big\"",
    ]);
    assert!(out.status.success(), "{out:?}");
    common::wait_for("the footprint record", || recorded(&w.remote).is_some());
    let idle = w.remote.join("cache/idle");
    std::fs::create_dir_all(idle.join("tree-0")).unwrap();
    std::fs::write(
        idle.join("meta.json"),
        r#"{"kind":"cache","repo":"idle","repo_id":"id-idle"}"#,
    )
    .unwrap();
    std::fs::write(idle.join("tree-0/big"), vec![0u8; 1 << 20]).unwrap();
    std::fs::write(idle.join("target-0.lock"), "").unwrap();
    fake_df(&w, 100 * MIB, 10 * GIB);
    // Pinned, so the scheduler does not hold the host back: the run itself makes room.
    let out = w.run(&["run", "--host", "local", "--", "true"]);
    let err = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(out.status.success(), "{err}");
    assert!(err.contains("the disk budget freed"), "{err}");
    assert!(!idle.join("tree-0").exists(), "{err}");
}
