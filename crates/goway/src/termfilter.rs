//! A streaming filter for remote command output shown on a terminal.
//!
//! The command runs on another machine (possibly a hostile repository's
//! code or a compromised helper); what it prints must not be able to drive
//! the user's terminal (clipboard writes, title changes, answerback
//! requests, cursor games). The filter keeps text, `\n`, `\r`, `\t`, `\b`
//! and SGR color sequences (`CSI ... m`) and drops everything else: OSC,
//! DCS, APC, PM and SOS strings, every other CSI and ESC sequence, and the
//! remaining C0 and C1 controls. It keeps its state between calls, so a
//! sequence split across two reads is handled the same as a whole one.

use std::io::Read;

/// How remote output is treated when it is shown on a terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, clap::ValueEnum)]
pub enum OutputMode {
    /// Strip terminal control sequences except colors (the default).
    #[default]
    Safe,
    /// Pass every byte through untouched.
    Raw,
}

/// Longest CSI parameter text kept while looking for the final byte.
const MAX_CSI: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Ground,
    /// After ESC.
    Esc,
    /// After ESC and one or more intermediate bytes (such as `ESC ( B`).
    EscInter,
    /// Inside CSI, collecting parameters.
    Csi,
    /// Inside an OSC, DCS, APC, PM or SOS string, until BEL or ST.
    Str,
    /// ESC seen inside a string (maybe the start of ST).
    StrEsc,
    /// After a 0xC2 byte (a possible C1 control in UTF-8).
    C2,
}

/// The stateful filter; feed it chunks with [`Filter::push`].
#[derive(Debug)]
pub struct Filter {
    state: State,
    csi: Vec<u8>,
    csi_ok: bool,
}

impl Default for Filter {
    fn default() -> Self {
        Self::new()
    }
}

impl Filter {
    /// A filter at the start of a stream.
    pub fn new() -> Self {
        Self {
            state: State::Ground,
            csi: Vec::new(),
            csi_ok: true,
        }
    }

    /// Filter `input`, appending what may be shown to `out`.
    pub fn push(&mut self, input: &[u8], out: &mut Vec<u8>) {
        for &b in input {
            self.byte(b, out);
        }
    }

    fn byte(&mut self, b: u8, out: &mut Vec<u8>) {
        match self.state {
            State::Ground => self.ground(b, out),
            State::C2 => {
                self.state = State::Ground;
                if (0x80..=0x9f).contains(&b) {
                    tracing::trace!(byte = b, "dropped C1 control");
                } else {
                    out.push(0xc2);
                    self.ground(b, out);
                }
            }
            State::Esc => match b {
                b'[' => {
                    self.state = State::Csi;
                    self.csi.clear();
                    self.csi_ok = true;
                }
                b']' | b'P' | b'X' | b'^' | b'_' => self.state = State::Str,
                0x20..=0x2f => self.state = State::EscInter,
                0x1b => {}
                _ => self.state = State::Ground,
            },
            State::EscInter => match b {
                0x20..=0x2f => {}
                0x1b => self.state = State::Esc,
                _ => self.state = State::Ground,
            },
            State::Csi => match b {
                0x1b => self.state = State::Esc,
                0x18 | 0x1a => self.state = State::Ground,
                0x30..=0x3f => self.csi_param(b),
                0x20..=0x2f => self.csi_ok = false,
                0x40..=0x7e => {
                    self.state = State::Ground;
                    if b == b'm' && self.csi_ok {
                        out.extend_from_slice(b"\x1b[");
                        out.extend_from_slice(&self.csi);
                        out.push(b'm');
                    }
                }
                // A control byte or a non-ASCII byte ends the sequence.
                _ => {
                    self.state = State::Ground;
                    self.ground(b, out);
                }
            },
            State::Str => match b {
                0x07 | 0x18 | 0x1a => self.state = State::Ground,
                0x1b => self.state = State::StrEsc,
                _ => {}
            },
            State::StrEsc => {
                if b == b'\\' {
                    self.state = State::Ground;
                } else {
                    self.state = State::Esc;
                    self.byte(b, out);
                }
            }
        }
    }

    /// One CSI parameter byte: only digits, `;` and `:` can form an SGR.
    fn csi_param(&mut self, b: u8) {
        if !(b.is_ascii_digit() || b == b';' || b == b':') || self.csi.len() >= MAX_CSI {
            self.csi_ok = false;
        } else {
            self.csi.push(b);
        }
    }

    fn ground(&mut self, b: u8, out: &mut Vec<u8>) {
        match b {
            0x1b => self.state = State::Esc,
            0xc2 => self.state = State::C2,
            b'\n' | b'\r' | b'\t' | 0x08 => out.push(b),
            0x00..=0x1f | 0x7f => tracing::trace!(byte = b, "dropped control byte"),
            _ => out.push(b),
        }
    }
}

/// Copy `reader` to our stdout or stderr through a [`Filter`] until EOF.
pub fn relay(mut reader: impl Read, to_stderr: bool) {
    let mut filter = Filter::new();
    let mut buf = [0u8; 8192];
    let mut out = Vec::with_capacity(buf.len());
    loop {
        match reader.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                out.clear();
                filter.push(&buf[..n], &mut out);
                if !out.is_empty() {
                    crate::render::passthrough(to_stderr, &out);
                }
            }
        }
    }
}

/// True when a stream should be filtered: it is a terminal and the mode is not raw.
pub fn should_filter(mode: OutputMode, is_terminal: bool) -> bool {
    mode == OutputMode::Safe && is_terminal
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(chunks: &[&[u8]]) -> Vec<u8> {
        let mut f = Filter::new();
        let mut out = Vec::new();
        for c in chunks {
            f.push(c, &mut out);
        }
        out
    }

    #[test]
    fn keeps_text_colors_and_the_five_allowed_controls() {
        let input = b"a\tb\r\n\x08\x1b[1;31mred\x1b[0m \x1b[38:5:9m \xc3\xa9\xe2\x9c\x93";
        assert_eq!(run(&[input]), input);
    }

    #[test]
    fn strips_osc_clipboard_title_and_queries() {
        let evil = b"x\x1b]52;c;Y3VybA==\x07\x1b]0;pwned\x1b\\\x1b[21t\x1b[6n\x1b[c\x1bP$qm\x1b\\y";
        assert_eq!(run(&[evil]), b"xy");
        assert_eq!(run(&[b"\x1b_apc\x1b\\\x1b^pm\x07\x1bXsos\x1b\\z"]), b"z");
    }

    #[test]
    fn strips_other_csi_esc_and_controls() {
        assert_eq!(
            run(&[b"a\x1b[2Jb\x1b[?1049hc\x1b[1;2Hd\x1b[0 qe"]),
            b"abcde"
        );
        assert_eq!(run(&[b"a\x1bcb\x1b(Bc\x07d\x00e\x7ff"]), b"abcdef");
        // Private-prefixed or intermediate-carrying `m` is not an SGR.
        assert_eq!(run(&[b"a\x1b[?1mb\x1b[1 mc\x1b[>4;2md"]), b"abcd");
        // C1 controls encoded in UTF-8 (CSI, OSC, DCS).
        assert_eq!(run(&[b"a\xc2\x9b2Jb\xc2\x9d0;x\xc2\x9cc"]), b"a2Jb0;xc");
    }

    #[test]
    fn handles_sequences_split_across_chunks_at_every_offset() {
        let input: &[u8] = b"p\x1b[31mq\x1b]52;c;QQ==\x07r\x1b[2Js\xc2\x9bt\x1bP1\x1b\\u";
        let whole = run(&[input]);
        assert_eq!(whole, b"p\x1b[31mqrstu");
        for i in 0..=input.len() {
            assert_eq!(run(&[&input[..i], &input[i..]]), whole, "split at {i}");
        }
        let bytes: Vec<&[u8]> = input.chunks(1).collect();
        assert_eq!(run(&bytes), whole);
    }

    #[test]
    fn an_escape_inside_a_string_starts_a_new_sequence() {
        assert_eq!(run(&[b"\x1b]0;t\x1b[31mred"]), b"\x1b[31mred");
    }

    #[test]
    fn never_emits_an_escape_that_is_not_sgr() {
        // Deterministic pseudo-random bytes biased towards control bytes.
        let alphabet = b"\x1b[]P_X^\\\x07\x18\x1a0123456789;:?m t\xc2\x9bA\n";
        let mut seed = 0x2545_f491_u32;
        let mut input = Vec::new();
        for _ in 0..20_000 {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            input.push(alphabet[(seed >> 16) as usize % alphabet.len()]);
        }
        let out = run(&[&input]);
        let mut i = 0;
        while i < out.len() {
            if out[i] == 0x1b {
                assert_eq!(out.get(i + 1), Some(&b'['), "bare ESC at {i}");
                let end = out[i + 2..]
                    .iter()
                    .position(|b| !(b.is_ascii_digit() || *b == b';' || *b == b':'));
                let end = i + 2 + end.expect("unterminated SGR");
                assert_eq!(out[end], b'm', "non-SGR CSI at {i}");
                i = end;
            } else {
                assert!(
                    !matches!(out[i], 0x00..=0x07 | 0x0b..=0x0c | 0x0e..=0x1a | 0x1c..=0x1f | 0x7f),
                    "control {:#x}",
                    out[i]
                );
            }
            i += 1;
        }
    }

    #[test]
    fn raw_mode_and_plain_streams_are_not_filtered() {
        assert!(should_filter(OutputMode::Safe, true));
        assert!(!should_filter(OutputMode::Raw, true));
        assert!(!should_filter(OutputMode::Safe, false));
    }
}
