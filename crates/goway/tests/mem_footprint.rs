//! Memory peaks: each helper records the most memory a repository's job
//! used, reports it in its probe, and explains a job the kernel's OOM killer
//! killed.
#![cfg(unix)]

mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::Path;

const MEBI: u64 = 1 << 20;

fn mempeak_files(root: &Path) -> Vec<std::path::PathBuf> {
    std::fs::read_dir(root.join("mempeaks"))
        .map(|d| d.filter_map(|e| e.ok().map(|e| e.path())).collect())
        .unwrap_or_default()
}

fn recorded(root: &Path) -> Option<u64> {
    let f = mempeak_files(root).into_iter().next()?;
    std::fs::read_to_string(f).ok()?.trim().parse().ok()
}

/// A `dmesg` that prints `text`.
fn fake_dmesg(w: &common::World, text: &str) {
    let path = w.bin.join("dmesg");
    std::fs::write(&path, format!("#!/bin/sh\necho '{text}'\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn probe(w: &common::World) -> String {
    let home = w.root.join("probe-home");
    std::fs::create_dir_all(&home).unwrap();
    let out = std::process::Command::new("bash")
        .args(["-c", include_str!("../src/remote.sh"), "goway", "probe"])
        .arg(&w.remote)
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

// frob:ticket 01M43CWNW1JNQZMCJBQ2NH4FTC
// frob:tests crates/goway/src/footprint.rs::parse_mem_peaks
#[test]
fn a_run_records_its_peak_memory_and_the_probe_reports_it() {
    let w = common::world();
    // A shell variable of 40 MB, held long enough for the sampler to see it.
    let out = w.run(&[
        "run",
        "--",
        "sh",
        "-c",
        "x=$(head -c 40000000 /dev/zero | tr '\\0' a); sleep 2; echo ${#x}",
    ]);
    assert!(out.status.success(), "{out:?}");
    common::wait_for("the memory record", || recorded(&w.remote).is_some());
    let peak = recorded(&w.remote).unwrap();
    assert!(peak >= 30 * MEBI, "peak {peak}");
    let text = probe(&w);
    assert!(text.contains("mempeak."), "{text}");
}

// frob:ticket 01M43CWNW1JNQZMCJBQ2NH4FTC
// frob:tests crates/goway/src/footprint.rs::mem_short_text
#[test]
fn a_job_the_oom_killer_killed_is_explained_in_plain_words() {
    let w = common::world();
    fake_dmesg(&w, "Out of memory: Killed process 4242 (rustc)");
    let out = w.run(&["run", "--", "sh", "-c", "sleep 1; kill -KILL $$"]);
    let err = String::from_utf8_lossy(&out.stderr).into_owned();
    assert_eq!(out.status.code(), Some(137), "{err}");
    assert!(err.contains("this host ran out of memory"), "{err}");
    assert!(err.contains("--needs mem>="), "{err}");
    assert!(err.contains("CARGO_BUILD_JOBS"), "{err}");
    assert!(err.contains("goway-setup tune"), "{err}");
    // The killed run's peak is on record, with room for what it never got to use.
    assert!(recorded(&w.remote).is_some());

    // The same exit without the kernel's word for it is just the job's own.
    let w2 = common::world();
    fake_dmesg(&w2, "usb 1-1: new device");
    let out = w2.run(&["run", "--", "sh", "-c", "sleep 1; kill -KILL $$"]);
    let err = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(!err.contains("ran out of memory"), "{err}");
}

// frob:ticket 01M43CWNW1JNQZMCJBQ2NH4FTC
// frob:tests crates/goway/src/footprint.rs::parse_mem_peaks
#[test]
fn a_job_in_its_own_scope_gets_its_command_line_untouched() {
    let w = common::world();
    // systemd-run expands `$$` to `$` in the arguments it is given; the fake does the same.
    let fake = w.bin.join("systemd-run");
    std::fs::write(
        &fake,
        "#!/bin/bash\nwhile [ \"${1#--}\" != \"$1\" ]; do shift; done\nargs=()\nfor a in \"$@\"; do args+=(\"${a//\\$\\$/\\$}\"); done\nexec \"${args[@]}\"\n",
    )
    .unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    let out = w.run(&["run", "--", "sh", "-c", "kill -TERM $$"]);
    assert_eq!(out.status.code(), Some(143), "{out:?}");
    let out = w.run(&["run", "--", "sh", "-c", "echo pid=$$ pct=%h"]);
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(text.contains("pct=%h") && !text.contains("pid=$"), "{text}");
}
