//! Security audit 3, H1: nothing a repository selects may run on the laptop.
//! A rustup-style shim that execs whatever `rust-toolchain.toml` in its
//! working directory names, and a `core.fsmonitor` program in the repo's own
//! git config, must never start when goway reads the project.
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;

mod common;

use common::{FAKE_SSH, world_with_ssh};

fn script(path: &Path, body: &str) {
    std::fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// Plant the repository-controlled launchers and return the marker they write.
fn plant(w: &common::World) -> std::path::PathBuf {
    let marker = w.root.join("PWNED");
    // What the rustup shim does: look for an override in the current
    // directory and run the program it names.
    let shim = format!(
        "[ -f rust-toolchain.toml ] && echo \"shim ran in $PWD\" >> '{m}'\necho 'rustc 1.80.0 (shim)'",
        m = marker.display()
    );
    script(&w.bin.join("cargo"), &shim);
    script(&w.bin.join("rustc"), &shim);
    std::fs::write(
        w.repo.join("Cargo.toml"),
        "[package]\nname = \"fx\"\nversion = \"0.0.0\"\nedition = \"2021\"\nrust-version = \"1.70\"\n",
    )
    .unwrap();
    std::fs::write(
        w.repo.join("rust-toolchain.toml"),
        "[toolchain]\npath = \"/proc/self/cwd/evil\"\n",
    )
    .unwrap();
    // The repo's own git config names a program for fsmonitor and hooks.
    let hook = w.repo.join("evil-hook.sh");
    script(
        &hook,
        &format!("echo \"git hook ran\" >> '{}'", marker.display()),
    );
    common::git(
        &w.repo,
        &["config", "core.fsmonitor", hook.to_str().unwrap()],
    );
    common::git(
        &w.repo,
        &["config", "core.hooksPath", w.repo.to_str().unwrap()],
    );
    marker
}

#[test]
fn project_tools_never_run_repo_code() {
    // A real ssh session starts in the helper's home, not in the laptop's
    // repository: the fake one must not start the "remote" probes there.
    let w = world_with_ssh(&FAKE_SSH.replace("exec sh -c", "cd \"$(dirname \"$0\")\"\nexec sh -c"));
    let marker = plant(&w);

    // The Rust requirement comes from the manifest, not from cargo.
    let got = goway::ecotools::rust_min(&w.repo).unwrap();
    assert_eq!(got.min, [1, 70]);

    // `goway run` (stale tool cache, so it reads the project's needs) and
    // `goway doctor`, started from inside the repository.
    let run = w.run(&["run", "--", "true"]);
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let _ = w.run(&["doctor", "local", "--all"]);

    // Git calls goway makes on an untrusted repo are hardened.
    let _ = goway::repo::git(&w.repo, &["status", "--porcelain"]);
    let _ = goway::sync::file_set(&w.repo, &goway::sync::Secrets::default());

    let ran = std::fs::read_to_string(&marker).unwrap_or_default();
    assert!(ran.is_empty(), "repository-controlled program ran: {ran}");
}
