//! scripts/install.sh then scripts/uninstall.sh must leave `$HOME` exactly
//! as it was: same files, same contents, same modes.
#![cfg(unix)]

use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Path to (kind, mode, content) for everything under `dir`.
fn snapshot(dir: &Path) -> BTreeMap<String, (char, u32, Vec<u8>)> {
    let mut out = BTreeMap::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap() {
            let p = e.unwrap().path();
            let meta = std::fs::symlink_metadata(&p).unwrap();
            let rel = p.strip_prefix(dir).unwrap().display().to_string();
            let mode = meta.permissions().mode();
            if meta.is_dir() {
                out.insert(rel, ('d', mode, Vec::new()));
                stack.push(p);
            } else {
                out.insert(rel, ('f', mode, std::fs::read(&p).unwrap()));
            }
        }
    }
    out
}

fn script(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../scripts")
        .join(name)
}

fn run(name: &str, home: &Path, path: &str) -> std::process::Output {
    let out = Command::new("bash")
        .arg(script(name))
        .env_clear()
        .env("HOME", home)
        .env("PATH", path)
        .env("GOWAY_INSTALL_BINARY", env!("CARGO_BIN_EXE_goway"))
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{name}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    out
}

const SYS_PATH: &str = "/usr/local/bin:/usr/bin:/bin";

fn round_trip(home: &Path, path: &str) {
    let before = snapshot(home);
    run("install.sh", home, path);
    let installed = home.join(".local/bin/goway");
    let version = Command::new(&installed).arg("--version").output().unwrap();
    assert!(String::from_utf8_lossy(&version.stdout).starts_with("goway "));
    run("uninstall.sh", home, path);
    assert_eq!(
        snapshot(home),
        before,
        "uninstall must restore $HOME exactly"
    );
}

#[test]
fn install_then_uninstall_restores_an_empty_home() {
    let home = tempfile::tempdir().unwrap();
    let before = snapshot(home.path());
    run("install.sh", home.path(), SYS_PATH);
    let profile = std::fs::read_to_string(home.path().join(".profile")).unwrap();
    assert!(
        profile.contains("$PATH:") && profile.contains(".local/bin\" # added by goway install"),
        "{profile}"
    );
    run("uninstall.sh", home.path(), SYS_PATH);
    assert_eq!(snapshot(home.path()), before);
}

#[test]
fn install_then_uninstall_keeps_existing_profile_and_dirs() {
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(home.path().join(".local/bin")).unwrap();
    std::fs::write(home.path().join(".local/bin/other"), "x").unwrap();
    // A profile without a trailing newline, mode 600.
    let profile = home.path().join(".profile");
    std::fs::write(&profile, "export EDITOR=vi").unwrap();
    std::fs::set_permissions(&profile, std::fs::Permissions::from_mode(0o600)).unwrap();
    round_trip(home.path(), SYS_PATH);
}

#[test]
fn bin_already_on_path_means_no_profile_edit() {
    let home = tempfile::tempdir().unwrap();
    let path = format!("{}/.local/bin:{SYS_PATH}", home.path().display());
    run("install.sh", home.path(), &path);
    assert!(!home.path().join(".profile").exists());
    run("uninstall.sh", home.path(), &path);
    assert!(snapshot(home.path()).is_empty());
}

#[test]
fn a_changed_binary_is_kept_and_a_second_install_is_refused() {
    let home = tempfile::tempdir().unwrap();
    run("install.sh", home.path(), SYS_PATH);
    let again = Command::new("bash")
        .arg(script("install.sh"))
        .env_clear()
        .env("HOME", home.path())
        .env("PATH", SYS_PATH)
        .env("GOWAY_INSTALL_BINARY", env!("CARGO_BIN_EXE_goway"))
        .output()
        .unwrap();
    assert!(
        !again.status.success(),
        "second install refused while the journal exists"
    );
    let bin = home.path().join(".local/bin/goway");
    std::fs::write(&bin, "user replaced it").unwrap();
    run("uninstall.sh", home.path(), SYS_PATH);
    assert_eq!(std::fs::read_to_string(&bin).unwrap(), "user replaced it");
    assert!(!home.path().join(".profile").exists());
}

#[test]
fn a_prefix_that_could_run_code_is_refused() {
    let home = tempfile::tempdir().unwrap();
    let out = Command::new("bash")
        .arg(script("install.sh"))
        .env_clear()
        .env("HOME", home.path())
        .env("PATH", SYS_PATH)
        .env(
            "GOWAY_PREFIX",
            format!("{}/x$(touch pwned)", home.path().display()),
        )
        .env("GOWAY_INSTALL_BINARY", env!("CARGO_BIN_EXE_goway"))
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(!home.path().join(".profile").exists());
    assert!(!home.path().join("pwned").exists());
}

// frob:tests crates/goway/src/uninstall.rs::revert_install_journal
#[test]
fn goway_uninstall_reverts_the_install_journal_exactly() {
    let home = tempfile::tempdir().unwrap();
    let profile = home.path().join(".profile");
    std::fs::write(&profile, "export EDITOR=vi").unwrap();
    let before = snapshot(home.path());
    run("install.sh", home.path(), SYS_PATH);
    let installed = home.path().join(".local/bin/goway");
    let out = Command::new(&installed)
        .args(["--color", "never", "uninstall", "--everywhere"])
        .env_clear()
        .env("HOME", home.path())
        .env("PATH", SYS_PATH)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(snapshot(home.path()), before, "home restored exactly");
}

/// The release target install.sh picks on the machine running the tests.
fn host_target() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "aarch64-apple-darwin",
        ("macos", _) => "x86_64-apple-darwin",
        (_, "aarch64") => "aarch64-unknown-linux-musl",
        _ => "x86_64-unknown-linux-musl",
    }
}

/// SHA-256 of a file by whichever of sha256sum and shasum the machine has.
fn sha256_of(file: &Path) -> String {
    let out = Command::new("sha256sum")
        .arg(file)
        .output()
        .or_else(|_| {
            Command::new("shasum")
                .args(["-a", "256"])
                .arg(file)
                .output()
        })
        .unwrap();
    String::from_utf8_lossy(&out.stdout)
        .split(' ')
        .next()
        .unwrap()
        .to_owned()
}

/// A fake release directory with the current goway binary, like CI makes.
fn fake_release(dir: &Path, tamper: bool) -> String {
    fake_release_for(dir, tamper, host_target())
}

/// A fake release directory whose archive is named for `target`.
fn fake_release_for(dir: &Path, tamper: bool, target: &str) -> String {
    let stage = dir.join("stage");
    std::fs::create_dir_all(&stage).unwrap();
    std::fs::copy(env!("CARGO_BIN_EXE_goway"), stage.join("goway")).unwrap();
    let tarball = format!("goway-{target}.tar.gz");
    let ok = Command::new("tar")
        .arg("-czf")
        .arg(dir.join(&tarball))
        .arg("-C")
        .arg(&stage)
        .arg("goway")
        .status()
        .unwrap();
    assert!(ok.success());
    let mut hex = sha256_of(&dir.join(&tarball));
    if tamper {
        hex = "0".repeat(64);
    }
    std::fs::write(dir.join("SHA256SUMS"), format!("{hex}  {tarball}\n")).unwrap();
    format!("file://{}", dir.display())
}

/// Run install.sh the way `curl ... | bash` does: from stdin, not a checkout.
fn install_piped(home: &Path, release: &str) -> std::process::Output {
    install_piped_with_path(home, release, SYS_PATH)
}

/// Like `install_piped`, with an explicit PATH (to stub uname or hide tools).
fn install_piped_with_path(home: &Path, release: &str, path: &str) -> std::process::Output {
    let script = std::fs::read(script("install.sh")).unwrap();
    let mut child = Command::new(find_tool("bash"))
        .arg("-s")
        .env_clear()
        .env("HOME", home)
        .env("PATH", path)
        .env("GOWAY_RELEASE_URL", release)
        .current_dir(home)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    std::io::Write::write_all(&mut child.stdin.take().unwrap(), &script).unwrap();
    child.wait_with_output().unwrap()
}

#[test]
fn piped_install_downloads_verifies_and_installs() {
    let home = tempfile::tempdir().unwrap();
    let release = tempfile::tempdir().unwrap();
    let url = fake_release(release.path(), false);
    let out = install_piped(home.path(), &url);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stderr).contains("checksum verified"));
    let version = Command::new(home.path().join(".local/bin/goway"))
        .arg("--version")
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&version.stdout).starts_with("goway "));
}

#[test]
fn a_checksum_mismatch_installs_nothing() {
    let home = tempfile::tempdir().unwrap();
    let release = tempfile::tempdir().unwrap();
    let url = fake_release(release.path(), true);
    let out = install_piped(home.path(), &url);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("checksum mismatch"));
    assert!(!home.path().join(".local/bin/goway").exists());
}

// A `curl | bash` cut off at any point must run nothing: the script is one
// function called on its last line.
#[test]
fn a_truncated_download_leaves_no_state() {
    let home = tempfile::tempdir().unwrap();
    let full = std::fs::read_to_string(script("install.sh")).unwrap();
    let lines: Vec<&str> = full.lines().collect();
    let before = snapshot(home.path());
    for keep in 1..lines.len() {
        let partial = lines[..keep].join("\n");
        let mut child = Command::new("bash")
            .arg("-s")
            .env_clear()
            .env("HOME", home.path())
            .env("PATH", SYS_PATH)
            .env("GOWAY_INSTALL_BINARY", env!("CARGO_BIN_EXE_goway"))
            .current_dir(home.path())
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        std::io::Write::write_all(&mut child.stdin.take().unwrap(), partial.as_bytes()).unwrap();
        let _ = child.wait();
        assert_eq!(
            snapshot(home.path()),
            before,
            "script cut after line {keep} left state"
        );
    }
}

#[test]
fn a_plain_http_release_url_is_refused() {
    let home = tempfile::tempdir().unwrap();
    let out = install_piped(home.path(), "http://example.invalid/release");
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("must start with https://"));
    assert!(!home.path().join(".local/bin/goway").exists());
}

#[test]
fn a_journal_left_by_an_aborted_install_is_cleared() {
    let home = tempfile::tempdir().unwrap();
    let state = home.path().join(".local/state/goway");
    std::fs::create_dir_all(&state).unwrap();
    std::fs::write(
        state.join("install-journal"),
        format!(
            "dir {}\ndir {}\n",
            home.path().join(".local/state").display(),
            state.display()
        ),
    )
    .unwrap();
    run("install.sh", home.path(), SYS_PATH);
    assert!(home.path().join(".local/bin/goway").exists());
    run("uninstall.sh", home.path(), SYS_PATH);
}

/// Write an executable shell stub `name` into `dir`.
fn stub(dir: &Path, name: &str, body: &str) {
    let file = dir.join(name);
    std::fs::write(&file, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// Absolute path of `tool` on the current PATH.
fn find_tool(tool: &str) -> PathBuf {
    std::env::split_paths(&std::env::var_os("PATH").unwrap())
        .map(|d| d.join(tool))
        .find(|p| p.is_file())
        .unwrap_or_else(|| panic!("{tool} not on PATH"))
}

// frob:tests scripts/install.sh
#[test]
fn macos_machines_get_the_apple_darwin_archive_for_their_cpu() {
    for (arch, target) in [
        ("arm64", "aarch64-apple-darwin"),
        ("x86_64", "x86_64-apple-darwin"),
    ] {
        let home = tempfile::tempdir().unwrap();
        let release = tempfile::tempdir().unwrap();
        let bin = tempfile::tempdir().unwrap();
        stub(
            bin.path(),
            "uname",
            &format!("case \"$1\" in -s) echo Darwin ;; -m) echo {arch} ;; esac"),
        );
        let url = fake_release_for(release.path(), false, target);
        let path = format!("{}:{SYS_PATH}", bin.path().display());
        let out = install_piped_with_path(home.path(), &url, &path);
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(out.status.success(), "{arch}: {err}");
        assert!(
            err.contains(&format!("downloading goway for {target}")),
            "{err}"
        );
        assert!(err.contains("checksum verified"), "{err}");
        assert!(home.path().join(".local/bin/goway").exists());
    }
}

// frob:tests scripts/install.sh
#[test]
fn shasum_verifies_the_download_when_sha256sum_is_missing() {
    let home = tempfile::tempdir().unwrap();
    let release = tempfile::tempdir().unwrap();
    let url = fake_release(release.path(), false);
    // A PATH holding only symlinks to what install.sh needs, plus a shasum
    // that is the machine's sha256 tool: no sha256sum is visible.
    let bin = tempfile::tempdir().unwrap();
    for tool in [
        "awk", "cat", "cmp", "curl", "cut", "dd", "dirname", "grep", "install", "mkdir", "mktemp",
        "od", "rm", "rmdir", "tail", "tar", "tr", "uname", "wc", "gzip",
    ] {
        std::os::unix::fs::symlink(find_tool(tool), bin.path().join(tool)).unwrap();
    }
    if std::env::consts::OS == "macos" {
        std::os::unix::fs::symlink("/usr/bin/shasum", bin.path().join("shasum")).unwrap();
    } else {
        let real = find_tool("sha256sum").display().to_string();
        stub(
            bin.path(),
            "shasum",
            &format!("[ \"$1\" = -a ] && shift 2\nexec {real} \"$@\""),
        );
    }
    let out = install_piped_with_path(home.path(), &url, &bin.path().display().to_string());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{err}");
    assert!(err.contains("checksum verified"), "{err}");
}
