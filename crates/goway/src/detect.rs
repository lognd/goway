//! Client side of test-binary detection for `goway run --shard N`.
//!
//! A shard command whose program is not a known tool may be a `GoogleTest`
//! or Catch2 v3 binary built on the helper. The helper decides (it reads
//! the file, never runs it; see `shard_run` in remote.sh) and ends the
//! run's stderr with one result line. This module asks for detection,
//! strips and parses that line, and remembers failed detections so later
//! runs of the same program in the same repository skip it.
//!
//! The result line is only ever used for the report, notes and the local
//! failure memory; no decision about rerunning is taken from it (the
//! helper reruns on its own, once, by an explicit attempt argument). Its
//! token carries a per-shard nonce, so a test that merely prints something
//! similar is not mistaken for it.

use std::io::Read;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use crate::state::State;

/// Byte that starts the result line (cannot be typed by accident).
const START: u8 = 0x01;
/// Longest result line accepted, newline included.
const MAX_LINE: usize = 512;

/// What the helper's detection did for one shard, as recorded in `--report`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[allow(clippy::struct_excessive_bools)] // one flag per documented outcome, serialized as is
pub struct Detection {
    /// `gtest`, `catch2` or `none`.
    pub detected: String,
    /// Exit code of each attempt (two when Catch2 was rerun).
    pub attempts: Vec<i32>,
    /// Catch2 rejected the shard flags and the shard ran once more without them.
    pub rerun: bool,
    /// The rerun was rejected as well; goway stopped there.
    pub rejected_again: bool,
    /// A failure that may be about the shard flags but was not certain; never rerun.
    pub flagged: bool,
    /// `GoogleTest` applied no sharding, so the shard ran the whole suite.
    pub duplicated: bool,
}

impl Detection {
    /// Whether detection failed in a way later runs should avoid.
    pub fn failed(&self) -> bool {
        self.rerun || self.duplicated
    }

    /// Parse the part of the result line after the nonce; `None` when malformed.
    pub fn parse(text: &str) -> Option<Self> {
        let mut d = Self {
            detected: String::new(),
            attempts: Vec::new(),
            rerun: false,
            rejected_again: false,
            flagged: false,
            duplicated: false,
        };
        let mut seen = 0;
        for word in text.split_whitespace() {
            let (key, value) = word.split_once('=')?;
            let flag = || match value {
                "0" => Some(false),
                "1" => Some(true),
                _ => None,
            };
            match key {
                "detected" if matches!(value, "gtest" | "catch2" | "none") => {
                    value.clone_into(&mut d.detected);
                }
                "attempts" => {
                    let codes: Option<Vec<i32>> =
                        value.split(',').map(|c| c.parse().ok()).collect();
                    d.attempts = codes.filter(|c| (1..=2).contains(&c.len()))?;
                }
                "rerun" => d.rerun = flag()?,
                "rejected" => d.rejected_again = flag()?,
                "flagged" => d.flagged = flag()?,
                "duplicated" => d.duplicated = flag()?,
                _ => return None,
            }
            seen += 1;
        }
        (seen == 6 && !d.detected.is_empty() && !d.attempts.is_empty()).then_some(d)
    }
}

/// A fresh nonce for one shard's result line (16 hex digits).
pub fn nonce() -> String {
    use std::hash::{BuildHasher as _, Hasher as _};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u128(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
    );
    h.write_u32(std::process::id());
    format!("{:016x}", h.finish())
}

/// The `run` word asking the helper to detect the framework of shard `index` of `count`.
pub fn request_word(index: usize, count: usize, nonce: &str) -> String {
    format!("shard-detect:{index}:{count}:{nonce}")
}

/// Wraps a shard's stderr: passes everything through unchanged except the
/// helper's result line (wherever it starts), which is removed and kept.
pub struct ResultSplitter<R> {
    inner: R,
    token: Vec<u8>,
    pending: Vec<u8>,
    ready: Vec<u8>,
    at: usize,
    eof: bool,
    found: Arc<Mutex<Option<String>>>,
}

impl<R: Read> ResultSplitter<R> {
    /// Wrap `inner`; the second value yields the result line text (after the nonce) once the stream ended.
    pub fn new(inner: R, nonce: &str) -> (Self, Arc<Mutex<Option<String>>>) {
        let found = Arc::new(Mutex::new(None));
        let mut token = vec![START];
        token.extend_from_slice(format!("goway-shard-result:{nonce} ").as_bytes());
        (
            Self {
                inner,
                token,
                pending: Vec::new(),
                ready: Vec::new(),
                at: 0,
                eof: false,
                found: Arc::clone(&found),
            },
            found,
        )
    }

    /// Move what can be decided from `pending` to `ready`.
    fn scan(&mut self) {
        let mut i = 0;
        while i < self.pending.len() {
            let Some(rel) = self.pending[i..].iter().position(|&b| b == START) else {
                i = self.pending.len();
                break;
            };
            let p = i + rel;
            let rest = &self.pending[p..];
            if rest.len() < self.token.len() {
                if self.token.starts_with(rest) && !self.eof {
                    // Could still become the token: wait for more bytes.
                    self.ready.extend_from_slice(&self.pending[..p]);
                    self.pending.drain(..p);
                    return;
                }
                i = p + 1;
                continue;
            }
            if !rest.starts_with(&self.token) {
                i = p + 1;
                continue;
            }
            match rest.iter().take(MAX_LINE).position(|&b| b == b'\n') {
                Some(nl) => {
                    let text = String::from_utf8_lossy(&rest[self.token.len()..nl]).into_owned();
                    tracing::debug!(%text, "shard result line");
                    *self.found.lock().expect("result lock") = Some(text);
                    self.ready.extend_from_slice(&self.pending[..p]);
                    self.pending.drain(..=p + nl);
                    i = 0;
                }
                None if rest.len() < MAX_LINE && !self.eof => {
                    self.ready.extend_from_slice(&self.pending[..p]);
                    self.pending.drain(..p);
                    return;
                }
                None => i = p + 1,
            }
        }
        self.ready
            .extend_from_slice(&self.pending[..i.min(self.pending.len())]);
        self.pending.drain(..i.min(self.pending.len()));
    }
}

impl<R: Read> Read for ResultSplitter<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        loop {
            if self.at < self.ready.len() {
                let n = (self.ready.len() - self.at).min(buf.len());
                buf[..n].copy_from_slice(&self.ready[self.at..self.at + n]);
                self.at += n;
                if self.at == self.ready.len() {
                    self.ready.clear();
                    self.at = 0;
                }
                return Ok(n);
            }
            if self.eof {
                return Ok(0);
            }
            let mut chunk = [0u8; 8192];
            let n = self.inner.read(&mut chunk)?;
            if n == 0 {
                self.eof = true;
            } else {
                self.pending.extend_from_slice(&chunk[..n]);
            }
            self.scan();
        }
    }
}

/// Whether detection already failed for `program` in repository `repo_id`.
pub fn is_marked(state: &State, repo_id: &str, program: &str) -> bool {
    state
        .shard_marks
        .get(repo_id)
        .is_some_and(|m| m.programs.iter().any(|p| p == program))
}

/// Remember that detection failed for `program` in repository `repo_id` (named `repo`).
pub fn mark(state: &mut State, repo: &str, repo_id: &str, program: &str) {
    tracing::info!(repo, program, "remembering a failed test-binary detection");
    let entry = state.shard_marks.entry(repo_id.to_owned()).or_default();
    repo.clone_into(&mut entry.repo);
    if !entry.programs.iter().any(|p| p == program) {
        entry.programs.push(program.to_owned());
    }
}

/// Forget the marks of the repository named or identified by `filter`; with no filter, all of them.
/// Returns how many programs were forgotten.
pub fn clear(state: &mut State, filter: Option<&str>) -> usize {
    let before: usize = state.shard_marks.values().map(|m| m.programs.len()).sum();
    state
        .shard_marks
        .retain(|id, m| filter.is_some_and(|f| f != id && f != m.repo));
    let after: usize = state.shard_marks.values().map(|m| m.programs.len()).sum();
    before - after
}

#[cfg(test)]
mod tests {
    use super::*;

    fn split(chunks: &[&[u8]], nonce: &str) -> (Vec<u8>, Option<String>) {
        struct Chunks<'a>(Vec<&'a [u8]>);
        impl Read for Chunks<'_> {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                if self.0.is_empty() {
                    return Ok(0);
                }
                let c = self.0.remove(0);
                buf[..c.len()].copy_from_slice(c);
                Ok(c.len())
            }
        }
        let (mut s, found) = ResultSplitter::new(Chunks(chunks.to_vec()), nonce);
        let mut out = Vec::new();
        s.read_to_end(&mut out).unwrap();
        let text = found.lock().unwrap().clone();
        (out, text)
    }

    // frob:tests crates/goway/src/detect.rs::ResultSplitter
    #[test]
    fn the_result_line_is_removed_wherever_it_starts_and_however_it_is_split() {
        let line = b"\x01goway-shard-result:abc detected=none attempts=0 rerun=0 rejected=0 flagged=0 duplicated=0\n";
        let (out, text) = split(&[b"before\npartial", line, b"after\n"], "abc");
        assert_eq!(out, b"before\npartialafter\n");
        assert!(text.unwrap().starts_with("detected=none"));
        // Split at every byte.
        let all: Vec<u8> = [&b"x\n"[..], line].concat();
        let chunks: Vec<&[u8]> = all.chunks(1).collect();
        let (out, text) = split(&chunks, "abc");
        assert_eq!(out, b"x\n");
        assert!(text.is_some());
    }

    #[test]
    fn a_line_with_another_nonce_or_no_newline_is_ordinary_output() {
        let forged = b"\x01goway-shard-result:zzz detected=gtest attempts=0 rerun=0 rejected=0 flagged=0 duplicated=0\n";
        let (out, text) = split(&[forged], "abc");
        assert_eq!(out, forged);
        assert!(text.is_none());
        let (out, text) = split(&[b"\x01goway-shard-result:abc detected=none"], "abc");
        assert_eq!(out, b"\x01goway-shard-result:abc detected=none");
        assert!(text.is_none());
        let (out, _) = split(&[b"\x01\x01\x01 odd bytes \x01g"], "abc");
        assert_eq!(out, b"\x01\x01\x01 odd bytes \x01g");
    }

    // frob:tests crates/goway/src/detect.rs::Detection
    #[test]
    fn result_lines_parse_strictly() {
        let d = Detection::parse(
            "detected=catch2 attempts=1,0 rerun=1 rejected=0 flagged=0 duplicated=0",
        )
        .unwrap();
        assert_eq!(d.attempts, [1, 0]);
        assert!(d.rerun && d.failed() && !d.rejected_again);
        for bad in [
            "",
            "detected=ruby attempts=0 rerun=0 rejected=0 flagged=0 duplicated=0",
            "detected=none attempts=0 rerun=2 rejected=0 flagged=0 duplicated=0",
            "detected=none attempts=0,1,2 rerun=0 rejected=0 flagged=0 duplicated=0",
            "detected=none attempts=0 rerun=0 rejected=0 flagged=0",
            "detected=none attempts=0 rerun=0 rejected=0 flagged=0 duplicated=0 extra=1",
        ] {
            assert!(Detection::parse(bad).is_none(), "{bad}");
        }
        assert!(
            !Detection::parse(
                "detected=gtest attempts=0 rerun=0 rejected=0 flagged=1 duplicated=0"
            )
            .unwrap()
            .failed(),
            "an uncertain failure is flagged, not remembered"
        );
    }

    // frob:tests crates/goway/src/detect.rs::clear
    #[test]
    fn marks_are_per_repository_program_and_cleared_by_repo() {
        let mut s = State::default();
        mark(&mut s, "alpha", "id1", "./build/tests");
        mark(&mut s, "alpha", "id1", "./build/tests");
        mark(&mut s, "beta", "id2", "./t");
        assert!(is_marked(&s, "id1", "./build/tests"));
        assert!(!is_marked(&s, "id1", "./t"));
        assert!(!is_marked(&s, "id2", "./build/tests"));
        assert_eq!(clear(&mut s, Some("alpha")), 1);
        assert!(!is_marked(&s, "id1", "./build/tests"));
        assert!(is_marked(&s, "id2", "./t"));
        assert_eq!(clear(&mut s, Some("id2")), 1);
        mark(&mut s, "a", "i", "p");
        assert_eq!(clear(&mut s, None), 1);
    }
}
