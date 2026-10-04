//! Shared test world: a scratch config, a git repository, and a fake `ssh`
//! on PATH that runs the "remote" command through a local shell.
#![allow(dead_code)] // each test binary uses a different subset

use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Longest any condition wait lasts: generous, because loaded machines are slow.
pub const WAIT: Duration = Duration::from_secs(120);

/// Poll `cond` every 20 ms until it holds, panicking after [`WAIT`].
pub fn wait_for(what: &str, mut cond: impl FnMut() -> bool) {
    let deadline = Instant::now() + WAIT;
    while !cond() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Collect everything `from` yields into the shared buffer, on a thread.
fn drain(mut from: impl std::io::Read + Send + 'static) -> Arc<Mutex<Vec<u8>>> {
    let buf = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&buf);
    std::thread::spawn(move || {
        let mut chunk = [0u8; 4096];
        while let Ok(n) = from.read(&mut chunk) {
            if n == 0 {
                break;
            }
            sink.lock().unwrap().extend_from_slice(&chunk[..n]);
        }
    });
    buf
}

/// A goway run whose command holds its slot (and GPU) until released, so a
/// test never guesses how long to sleep: it waits for the run to have started,
/// does its work, then releases it.
pub struct Held {
    child: Child,
    started: PathBuf,
    release: PathBuf,
    stdout: Arc<Mutex<Vec<u8>>>,
    stderr: Arc<Mutex<Vec<u8>>>,
}

static HELD: AtomicUsize = AtomicUsize::new(0);

impl Held {
    /// Wait until the command is running (it holds its slot from then on).
    pub fn wait_started(&self) {
        wait_for("the held run to start", || self.started.exists());
    }

    /// Wait until the run's stderr so far contains `text`.
    pub fn wait_stderr(&self, text: &str) {
        wait_for(&format!("stderr to contain {text:?}"), || {
            self.stderr().contains(text)
        });
    }

    /// Everything the run has written to stderr so far.
    pub fn stderr(&self) -> String {
        String::from_utf8_lossy(&self.stderr.lock().unwrap()).into_owned()
    }

    /// Let the command finish.
    pub fn release(&self) {
        std::fs::write(&self.release, "").unwrap();
    }

    /// Kill goway itself (the remote command keeps running until released).
    pub fn kill_client(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    /// Release, wait for the run to end and return what it printed.
    pub fn finish(mut self) -> Output {
        self.release();
        let status = self.child.wait().unwrap();
        // The pipes close with the process; give the readers a moment.
        wait_for("the output readers", || {
            Arc::strong_count(&self.stdout) == 1 && Arc::strong_count(&self.stderr) == 1
        });
        let out = self.stdout.lock().unwrap().clone();
        let err = self.stderr.lock().unwrap().clone();
        Output {
            status,
            stdout: out,
            stderr: err,
        }
    }
}

impl Drop for Held {
    fn drop(&mut self) {
        // A failed test must not leave the remote shell polling forever.
        let _ = std::fs::write(&self.release, "");
        let _ = self.child.kill();
    }
}

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
    // The fake remote is this machine: on WSL its first static probe would
    // otherwise run the real powershell.exe (seconds, unbounded under load).
    let powershell = bin.join("powershell.exe");
    std::fs::write(&powershell, "#!/bin/sh\nexit 0\n").unwrap();
    std::fs::set_permissions(&powershell, std::fs::Permissions::from_mode(0o755)).unwrap();
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
            .env_remove("GOWAY_HOST")
            .env_remove("GOWAY_DEPTH")
            .env_remove("GOWAY_CHAIN")
            .env_remove("GOWAY_SHARD")
            .env_remove("GOWAY_SHARD_COUNT");
        cmd
    }

    /// Start `goway run <run_args> -- sh -c '<body>; wait for release'` with
    /// piped output; the command touches a marker when it starts running.
    pub fn hold(&self, run_args: &[&str], body: &str) -> Held {
        let id = HELD.fetch_add(1, Ordering::Relaxed);
        let started = self.root.join(format!("held-{id}.started"));
        let release = self.root.join(format!("held-{id}.release"));
        let script = format!(
            "{body}; : > '{}'; while [ ! -e '{}' ]; do sleep 0.05; done",
            started.display(),
            release.display()
        );
        let mut args = vec!["run"];
        args.extend_from_slice(run_args);
        args.extend(["--", "sh", "-c", &script]);
        let mut child = self
            .goway(&args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let stdout = drain(child.stdout.take().unwrap());
        let stderr = drain(child.stderr.take().unwrap());
        Held {
            child,
            started,
            release,
            stdout,
            stderr,
        }
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
