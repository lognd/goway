//! End-to-end `goway run` without a network: a fake `ssh` on PATH runs the
//! remote command through a local shell, so the real binary, the real
//! remote script and the real sync protocol are exercised.
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// Ignores ssh options and runs the remote command line with `sh -c`.
const FAKE_SSH: &str = r#"#!/bin/sh
while [ $# -gt 0 ]; do
  case "$1" in
    -o|-p|-l) shift 2 ;;
    --) shift; shift; break ;;
    *) shift ;;
  esac
done
exec sh -c "$1"
"#;

struct World {
    _dir: tempfile::TempDir,
    root: PathBuf,
    bin: PathBuf,
    config: PathBuf,
    repo: PathBuf,
    remote: PathBuf,
}

fn git(dir: &Path, args: &[&str]) {
    let ok = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .status()
        .unwrap();
    assert!(ok.success(), "git {args:?}");
}

fn world() -> World {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let bin = root.join("bin");
    std::fs::create_dir(&bin).unwrap();
    let ssh = bin.join("ssh");
    std::fs::write(&ssh, FAKE_SSH).unwrap();
    std::fs::set_permissions(&ssh, std::fs::Permissions::from_mode(0o755)).unwrap();
    let remote = root.join("remote");
    let config = root.join("config");
    std::fs::create_dir(&config).unwrap();
    std::fs::write(
        config.join("config.toml"),
        format!(
            "[defaults]\nremote_root = \"{}\"\ntarget_slots = 2\n\n[[host]]\nname = \"local\"\naddress = \"127.0.0.1\"\n",
            remote.display()
        ),
    )
    .unwrap();
    let repo = root.join("proj");
    std::fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("hello.txt"), "hello\n").unwrap();
    World {
        _dir: dir,
        root,
        bin,
        config,
        repo,
        remote,
    }
}

impl World {
    fn goway(&self, args: &[&str]) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_goway"));
        let path = format!(
            "{}:{}",
            self.bin.display(),
            std::env::var("PATH").unwrap_or_default()
        );
        cmd.args(["--color", "never"])
            .args(args)
            .current_dir(&self.repo)
            .env("PATH", path)
            .env("GOWAY_CONFIG_DIR", &self.config)
            .env("GOWAY_STATE_DIR", self.root.join("state"))
            .env_remove("RUSTC_WRAPPER");
        cmd
    }

    fn run(&self, args: &[&str]) -> Output {
        self.goway(args).output().unwrap()
    }

    fn work_dirs(&self) -> Vec<String> {
        std::fs::read_dir(self.remote.join("work"))
            .map(|d| {
                d.map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default()
    }
}

#[test]
fn exit_code_passes_through_and_work_dir_is_removed() {
    let w = world();
    let out = w.run(&[
        "run",
        "--",
        "sh",
        "-c",
        "cat hello.txt; echo to-stderr >&2; exit 7",
    ]);
    assert_eq!(
        out.status.code(),
        Some(7),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout), "hello\n");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("to-stderr"), "{stderr}");
    assert!(stderr.contains("goway: running on local"), "{stderr}");
    assert!(w.work_dirs().is_empty(), "{:?}", w.work_dirs());

    let kept = w.run(&["run", "--keep", "--", "true"]);
    assert_eq!(kept.status.code(), Some(0));
    assert_eq!(w.work_dirs().len(), 1, "--keep keeps the work dir");
}

#[test]
fn cargo_target_dir_is_a_free_per_repo_slot() {
    let w = world();
    let print = ["run", "--", "sh", "-c", "echo $CARGO_TARGET_DIR"];
    let first = w.run(&print);
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let slot0 = String::from_utf8_lossy(&first.stdout).trim().to_owned();
    assert!(
        slot0.starts_with(&w.remote.join("cache").display().to_string()),
        "{slot0}"
    );
    assert!(slot0.ends_with("/target-0"), "{slot0}");

    // While one run holds slot 0, a concurrent run gets slot 1.
    let mut busy = w
        .goway(&["run", "--", "sh", "-c", "sleep 3"])
        .spawn()
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(1500));
    let second = w.run(&print);
    let slot = String::from_utf8_lossy(&second.stdout).trim().to_owned();
    assert!(busy.wait().unwrap().success());
    assert!(
        slot.ends_with("/target-1"),
        "{slot}\n{}",
        String::from_utf8_lossy(&second.stderr)
    );
    assert_eq!(Path::new(&slot).parent(), Path::new(&slot0).parent());
}

#[test]
fn report_names_host_arch_address_and_exit_code() {
    let w = world();
    let report = w.root.join("report.json");
    let out = w.run(&[
        "run",
        "--report",
        report.to_str().unwrap(),
        "--",
        "sh",
        "-c",
        "exit 3",
    ]);
    assert_eq!(out.status.code(), Some(3));
    let json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&report).unwrap()).unwrap();
    assert_eq!(json["host"], "local");
    assert_eq!(json["address"], "127.0.0.1");
    assert_eq!(json["exit_code"], 3);
    assert_eq!(json["arch"], std::env::consts::ARCH);
    assert!(json["hostname"].as_str().is_some_and(|h| !h.is_empty()));
}

#[test]
fn env_files_never_reach_the_remote() {
    let w = world();
    std::fs::write(w.repo.join(".env"), "SECRET=placeholder\n").unwrap();
    let out = w.run(&["run", "--", "sh", "-c", "ls -A"]);
    let listing = String::from_utf8_lossy(&out.stdout);
    assert!(listing.contains("hello.txt"), "{listing}");
    assert!(!listing.contains(".env"), "{listing}");
}

#[test]
fn signals_map_to_128_plus_n() {
    let w = world();
    let out = w.run(&["run", "--", "sh", "-c", "kill -TERM $$"]);
    assert_eq!(out.status.code(), Some(143));
}
