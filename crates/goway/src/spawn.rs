//! Spawning child processes from several threads without leaking pipes.
//!
//! On macOS (and other systems without `pipe2`), Rust creates a pipe and sets
//! close-on-exec on it in two steps. A child forked by another thread in
//! between inherits the pipe's write end, and the reader of that pipe then
//! never sees end-of-file until the unrelated child exits: a helper call's
//! answer is held up by the long-running job another thread just started.
//! Every spawn that can race with another thread goes through one lock, so
//! no fork happens while another spawn is setting up its pipes. That includes
//! the unpiped ones: any unlocked fork can carry a half-made pipe into a
//! long-lived child, so no production code spawns without the lock (a test
//! scans for it).

use std::io;
use std::process::{Child, Command, ExitStatus, Output, Stdio};
use std::sync::Mutex;

/// Held while a child is being created (pipes made, forked, exec'd).
static SPAWN: Mutex<()> = Mutex::new(());

/// Spawn variants that never race with another thread's pipe creation.
pub trait CommandExt {
    /// [`Command::spawn`] under the process-wide spawn lock.
    fn spawn_locked(&mut self) -> io::Result<Child>;

    /// [`Command::output`] (stdout and stderr captured) with the spawn under the lock.
    fn output_locked(&mut self) -> io::Result<Output>;

    /// [`Command::status`] with the spawn under the lock.
    fn status_locked(&mut self) -> io::Result<ExitStatus>;
}

impl CommandExt for Command {
    fn spawn_locked(&mut self) -> io::Result<Child> {
        // A poisoned lock only means another spawn panicked; the guard protects nothing else.
        let _guard = SPAWN
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let child = self.spawn();
        tracing::trace!(ok = child.is_ok(), "spawned a child under the spawn lock");
        child
    }

    fn output_locked(&mut self) -> io::Result<Output> {
        self.stdout(Stdio::piped()).stderr(Stdio::piped());
        self.spawn_locked()?.wait_with_output()
    }

    fn status_locked(&mut self) -> io::Result<ExitStatus> {
        self.spawn_locked()?.wait()
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    // frob:tests crates/goway/src/spawn.rs::CommandExt
    #[test]
    fn short_calls_are_not_held_up_by_children_other_threads_start() {
        let worst = std::thread::scope(|s| {
            let longs: Vec<_> = (0..4)
                .map(|_| {
                    s.spawn(|| {
                        for _ in 0..10 {
                            let mut c = Command::new("sleep")
                                .arg("3")
                                .stdin(Stdio::null())
                                .stdout(Stdio::null())
                                .stderr(Stdio::null())
                                .spawn_locked()
                                .unwrap();
                            c.kill().ok();
                            c.wait().ok();
                        }
                    })
                })
                .collect();
            let shorts: Vec<_> = (0..4)
                .map(|_| {
                    s.spawn(|| {
                        let mut worst = Duration::ZERO;
                        for _ in 0..40 {
                            let t = Instant::now();
                            let out = Command::new("echo").arg("hi").output_locked().unwrap();
                            assert_eq!(out.stdout, b"hi\n");
                            worst = worst.max(t.elapsed());
                        }
                        worst
                    })
                })
                .collect();
            for l in longs {
                l.join().unwrap();
            }
            shorts.into_iter().map(|t| t.join().unwrap()).max().unwrap()
        });
        assert!(worst < Duration::from_secs(2), "a call waited {worst:?}");
    }

    // frob:ticket 01M43QA57XZY29C6T6SE09XD8K
    // frob:tests crates/goway/src/spawn.rs::CommandExt
    #[test]
    fn no_production_code_spawns_a_child_outside_the_lock() {
        // One unlocked fork anywhere can hand another thread's half-made pipe
        // to a long-lived child (the lifeline, a job), which then holds that
        // pipe's reader up until the child exits: the macOS hang.
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut bad = Vec::new();
        for entry in std::fs::read_dir(&src).unwrap().flatten() {
            let path = entry.path();
            if path.extension().is_none_or(|e| e != "rs") || path.ends_with("spawn.rs") {
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap();
            let production = text.split("\n#[cfg(test)]").next().unwrap();
            for (n, line) in production.lines().enumerate() {
                if [".spawn()", ".output()", ".status()"]
                    .iter()
                    .any(|m| line.contains(m))
                {
                    bad.push(format!("{}:{}: {}", path.display(), n + 1, line.trim()));
                }
            }
        }
        assert!(
            bad.is_empty(),
            "use the *_locked variants:\n{}",
            bad.join("\n")
        );
    }
}
