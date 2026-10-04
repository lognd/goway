//! Spawning child processes from several threads without leaking pipes.
//!
//! On macOS (and other systems without `pipe2`), Rust creates a pipe and sets
//! close-on-exec on it in two steps. A child forked by another thread in
//! between inherits the pipe's write end, and the reader of that pipe then
//! never sees end-of-file until the unrelated child exits: a helper call's
//! answer is held up by the long-running job another thread just started.
//! Every spawn that can race with another thread goes through one lock, so
//! no fork happens while another spawn is setting up its pipes.

use std::io;
use std::process::{Child, Command, Output, Stdio};
use std::sync::Mutex;

/// Held while a child is being created (pipes made, forked, exec'd).
static SPAWN: Mutex<()> = Mutex::new(());

/// Spawn variants that never race with another thread's pipe creation.
pub trait CommandExt {
    /// [`Command::spawn`] under the process-wide spawn lock.
    fn spawn_locked(&mut self) -> io::Result<Child>;

    /// [`Command::output`] (stdout and stderr captured) with the spawn under the lock.
    fn output_locked(&mut self) -> io::Result<Output>;
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
}
