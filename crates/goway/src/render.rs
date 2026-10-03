//! The one module that prints. Everything goway itself says goes through a
//! [`Renderer`]; the streamed output of remote commands never does.
//!
//! Status lines, warnings and errors go to stderr so stdout stays clean for
//! machine-readable output and for the remote command's own stdout.
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
}

impl Renderer {
    /// Build a renderer honouring `--color` (and `NO_COLOR` under `auto`).
    pub fn new(when: ColorWhen) -> Self {
        let choice = match when {
            ColorWhen::Auto => ColorChoice::Auto,
            ColorWhen::Always => ColorChoice::Always,
            ColorWhen::Never => ColorChoice::Never,
        };
        Self { choice }
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
        let _ = writeln!(self.err(), "{ERROR}goway: error:{ERROR:#} {error}");
    }

    /// Report a non-fatal problem on stderr.
    pub fn warn(self, message: impl Display) {
        use std::io::Write as _;
        let _ = writeln!(self.err(), "{WARN}goway: warning:{WARN:#} {message}");
    }

    /// A progress or status note on stderr.
    pub fn note(self, message: impl Display) {
        use std::io::Write as _;
        let _ = writeln!(self.err(), "{DIM}goway:{DIM:#} {message}");
    }

    /// A success line on stderr.
    pub fn ok(self, message: impl Display) {
        use std::io::Write as _;
        let _ = writeln!(self.err(), "{GOOD}goway:{GOOD:#} {message}");
    }

    /// An accented headline on stderr, such as where a run is going.
    pub fn headline(self, message: impl Display) {
        use std::io::Write as _;
        let _ = writeln!(self.err(), "{ACCENT}goway:{ACCENT:#} {message}");
    }

    /// A line of primary output on stdout (tables, paths, lists).
    pub fn line(self, message: impl Display) {
        use std::io::Write as _;
        let _ = writeln!(self.out(), "{message}");
    }

    /// Render rows as an aligned table on stdout; the first row is the header.
    pub fn table(self, rows: &[Vec<String>]) {
        use std::io::Write as _;
        let mut out = self.out();
        for (i, row) in format_table(rows).iter().enumerate() {
            let _ = if i == 0 {
                writeln!(out, "{ACCENT}{row}{ACCENT:#}")
            } else {
                writeln!(out, "{row}")
            };
        }
    }
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

    #[test]
    fn table_columns_align() {
        let rows = vec![
            vec!["host".to_owned(), "load".to_owned()],
            vec!["helios".to_owned(), "0.1".to_owned()],
        ];
        assert_eq!(format_table(&rows), vec!["host    load", "helios  0.1"]);
    }
}
