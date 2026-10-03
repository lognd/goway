//! Shared test world: a scratch config, a git repository, and a fake `ssh`
//! on PATH that runs the "remote" command through a local shell.
#![allow(dead_code)] // each test binary uses a different subset

use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// Ignores ssh options and runs the remote command line with `sh -c`.
pub const FAKE_SSH: &str = r#"#!/bin/sh
while [ $# -gt 0 ]; do
  case "$1" in
    -o|-p|-l) shift 2 ;;
    --) shift; shift; break ;;
    *) shift ;;
  esac
done
exec sh -c "$1"
"#;

pub struct World {
    _dir: tempfile::TempDir,
    /// Scratch root.
    pub root: PathBuf,
    /// Holds the fake `ssh`.
    pub bin: PathBuf,
    /// goway's config dir.
    pub config: PathBuf,
    /// The local repository.
    pub repo: PathBuf,
    /// goway's remote root.
    pub remote: PathBuf,
}

pub fn git(dir: &Path, args: &[&str]) {
    let ok = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .status()
        .unwrap();
    assert!(ok.success(), "git {args:?}");
}

pub fn world() -> World {
    world_with_ssh(FAKE_SSH)
}

/// A world whose fake `ssh` is `script`.
pub fn world_with_ssh(script: &str) -> World {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let bin = root.join("bin");
    std::fs::create_dir(&bin).unwrap();
    let ssh = bin.join("ssh");
    std::fs::write(&ssh, script).unwrap();
    std::fs::set_permissions(&ssh, std::fs::Permissions::from_mode(0o755)).unwrap();
    let remote = root.join("goway-remote");
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
    pub fn goway(&self, args: &[&str]) -> Command {
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
            .env("GOWAY_WINDOWS_LOOKUP", "0")
            // The fake ssh reads these; real ssh never sees them.
            .env(
                "GOWAY_SSH_PASS_ENV",
                "FAKE_HOSTNAME,FAKE_WINDOWS_PORT,RUSTC_WRAPPER,CARGO_TARGET_DIR,GOWAY_WINDOWS_LOOKUP",
            )
            // The fake remote is this machine: settings inherited from an
            // outer goway job (or the user's shell) must not leak in.
            .env_remove("RUSTC_WRAPPER")
            .env_remove("CARGO_TARGET_DIR")
            .env_remove("SCCACHE_DIR")
            .env_remove("SCCACHE_SERVER_PORT")
            .env_remove("GOWAY")
            .env_remove("GOWAY_RUN_ID")
            .env_remove("GOWAY_SHARD")
            .env_remove("GOWAY_SHARD_COUNT");
        cmd
    }

    pub fn run(&self, args: &[&str]) -> Output {
        self.goway(args).output().unwrap()
    }

    pub fn work_dirs(&self) -> Vec<String> {
        std::fs::read_dir(self.remote.join("work"))
            .map(|d| {
                d.map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default()
    }
}
