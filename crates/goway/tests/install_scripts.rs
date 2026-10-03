//! scripts/install.sh then scripts/uninstall.sh must leave `$HOME` exactly
//! as it was: same files, same contents, same modes.
#![cfg(target_os = "linux")]

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
        profile.contains(".local/bin:$PATH\" # added by goway install"),
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
