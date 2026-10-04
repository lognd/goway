//! One PowerShell process per host instead of one per helper call.
//!
//! Starting `powershell.exe` costs a third of a second or more, and a warm
//! run makes half a dozen helper calls (manifest, receive, env, the copy
//! verification). On a Windows helper goway therefore starts `remote.ps1
//! session` once and sends the calls down its standard input as frames; the
//! answers come back the same way. The helper's verbs are unchanged. Any
//! failure of the session (it cannot start, dies, speaks nonsense, an input
//! is too large to buffer) makes the caller use one process per call, as
//! before.
//!
//! Frame to the helper: two big-endian `u32` (length of the arguments, length
//! of the input), the NUL-terminated arguments (verb first), the input.
//! Answer: three big-endian 32-bit values (exit code, stdout length, stderr
//! length), stdout, stderr. The helper first prints the line
//! `goway-session1`.

use std::collections::BTreeMap;
use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::{Arc, Mutex};

use crate::spawn::CommandExt as _;

/// The line the helper prints when its session loop is ready.
pub const READY: &str = "goway-session1";

/// Largest input goway buffers for one framed call; bigger ones (a first
/// sync of a large tree) use a process of their own.
pub const MAX_INPUT: usize = 32 << 20;

/// Set `GOWAY_NO_SESSION` to use one process per call.
const OFF_ENV: &str = "GOWAY_NO_SESSION";

/// What one framed call answered.
#[derive(Debug)]
pub struct Reply {
    /// The verb's exit code.
    pub code: i32,
    /// What it wrote to stdout.
    pub stdout: Vec<u8>,
    /// What it wrote to stderr.
    pub stderr: Vec<u8>,
}

/// A running helper session.
#[derive(Debug)]
pub struct Session {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

fn be32(n: usize) -> std::io::Result<[u8; 4]> {
    u32::try_from(n)
        .map(u32::to_be_bytes)
        .map_err(|_| std::io::Error::other("frame too large"))
}

impl Session {
    /// Spawn `cmd` (which runs `remote.ps1 session`) and wait for its ready line.
    ///
    /// # Errors
    ///
    /// The helper's stderr (or the spawn error) when it does not become ready.
    pub fn start(mut cmd: Command) -> std::result::Result<Self, String> {
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = cmd
            .spawn_locked()
            .map_err(|e| format!("cannot start: {e}"))?;
        let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
            return Err("no pipes".to_owned());
        };
        let mut stdout = BufReader::new(stdout);
        let mut line = String::new();
        let ready = stdout.read_line(&mut line).is_ok() && line.trim() == READY;
        if !ready {
            let mut err = String::new();
            if let Some(mut e) = child.stderr.take() {
                let _ = e.read_to_string(&mut err);
            }
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("{}{}", line.trim(), err.trim()));
        }
        Ok(Self {
            child,
            stdin,
            stdout,
        })
    }

    /// Run one verb (`words[0]`) with `input` on its stdin.
    ///
    /// # Errors
    ///
    /// An I/O error when the session broke or answered out of line; the
    /// session must then be dropped.
    pub fn call(
        &mut self,
        words: &[&str],
        input: &[u8],
        max_stdout: usize,
    ) -> std::io::Result<Reply> {
        let mut args = Vec::new();
        for w in words {
            args.extend_from_slice(w.as_bytes());
            args.push(0);
        }
        self.stdin.write_all(&be32(args.len())?)?;
        self.stdin.write_all(&be32(input.len())?)?;
        self.stdin.write_all(&args)?;
        self.stdin.write_all(input)?;
        self.stdin.flush()?;
        let mut head = [0u8; 12];
        self.stdout.read_exact(&mut head)?;
        let n = |i: usize| u32::from_be_bytes([head[i], head[i + 1], head[i + 2], head[i + 3]]);
        let code = i32::from_be_bytes([head[0], head[1], head[2], head[3]]);
        let (out_len, err_len) = (n(4) as usize, n(8) as usize);
        if out_len > max_stdout || err_len > crate::sync::MAX_HELPER_STDERR {
            return Err(std::io::Error::other("the helper's answer is too large"));
        }
        let mut stdout = vec![0u8; out_len];
        self.stdout.read_exact(&mut stdout)?;
        let mut stderr = vec![0u8; err_len];
        self.stdout.read_exact(&mut stderr)?;
        Ok(Reply {
            code,
            stdout,
            stderr,
        })
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // End of input ends the helper's loop; do not leave it running.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A host's session slot: not tried yet, live, or given up on.
#[derive(Debug, Default)]
enum Slot {
    #[default]
    Unstarted,
    Live(Session),
    Dead,
}

static SESSIONS: Mutex<BTreeMap<String, Arc<Mutex<Slot>>>> = Mutex::new(BTreeMap::new());

/// Whether sessions are switched off by the environment.
pub fn disabled() -> bool {
    std::env::var_os(OFF_ENV).is_some_and(|v| !v.is_empty() && v != "0")
}

/// Run the framed call on the session for `key`, starting it with `start`
/// (which should end in [`Session::start`]) first when needed. `None` means there is no usable session (never
/// started, failed to start, or broke): the caller uses a process of its own.
/// Otherwise the helper's answer.
pub fn call(
    key: &str,
    start: impl FnOnce() -> std::result::Result<Session, String>,
    words: &[&str],
    input: &[u8],
) -> Option<Reply> {
    if disabled() || input.len() > MAX_INPUT {
        return None;
    }
    let slot = {
        let mut all = SESSIONS.lock().ok()?;
        Arc::clone(all.entry(key.to_owned()).or_default())
    };
    let mut slot = slot.lock().ok()?;
    if matches!(*slot, Slot::Unstarted) {
        *slot = match start() {
            Ok(s) => {
                tracing::debug!(key, "helper session started");
                Slot::Live(s)
            }
            Err(e) => {
                tracing::debug!(key, error = %e, "no helper session; one process per call");
                Slot::Dead
            }
        };
    }
    let Slot::Live(session) = &mut *slot else {
        return None;
    };
    match session.call(words, input, crate::sync::MAX_HELPER_STDOUT) {
        Ok(reply) => Some(reply),
        Err(e) => {
            tracing::warn!(key, error = %e, "helper session broke; one process per call from here");
            *slot = Slot::Dead;
            None
        }
    }
}

/// End every session (their helpers stop at end of input).
pub fn close_all() {
    if let Ok(mut all) = SESSIONS.lock() {
        all.clear();
    }
}
