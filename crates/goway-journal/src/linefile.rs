//! Line-oriented text editing that round-trips bytes exactly, plus ini helpers.

/// A text file as lines plus whether the last line ends in a newline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LineFile {
    pub lines: Vec<String>,
    pub ends_nl: bool,
}

impl LineFile {
    /// Parse text; `render` is its exact inverse.
    pub fn parse(text: &str) -> Self {
        if text.is_empty() {
            return Self {
                lines: Vec::new(),
                ends_nl: false,
            };
        }
        let (body, ends_nl) = text.strip_suffix('\n').map_or((text, false), |b| (b, true));
        Self {
            lines: body.split('\n').map(str::to_owned).collect(),
            ends_nl,
        }
    }

    /// Serialize back to text.
    pub fn render(&self) -> String {
        if self.lines.is_empty() {
            return String::new();
        }
        let mut out = self.lines.join("\n");
        if self.ends_nl {
            out.push('\n');
        }
        out
    }

    /// Insert a line; returns true when a missing final newline had to be added.
    pub fn insert(&mut self, idx: usize, line: String) -> bool {
        let tail = idx == self.lines.len();
        let fixed = tail && !self.lines.is_empty() && !self.ends_nl;
        if tail && (self.lines.is_empty() || fixed) {
            self.ends_nl = true;
        }
        self.lines.insert(idx, line);
        fixed
    }

    /// Remove a line.
    pub fn remove(&mut self, idx: usize) {
        self.lines.remove(idx);
    }

    /// Undo a newline added by `insert` when nothing follows `idx` any more.
    pub fn unfix_newline(&mut self, idx: usize) {
        if idx >= self.lines.len() && !self.lines.is_empty() {
            self.ends_nl = false;
        }
    }
}

/// The section name of an ini header line.
pub(crate) fn header_name(line: &str) -> Option<&str> {
    let t = line.trim();
    t.strip_prefix('[')?.strip_suffix(']').map(str::trim)
}

/// Split a `key = value` line; comments and headers are not key lines.
pub(crate) fn parse_kv(line: &str) -> Option<(&str, &str)> {
    let t = line.trim();
    if t.starts_with(['#', ';', '[']) {
        return None;
    }
    let (k, v) = t.split_once('=')?;
    Some((k.trim(), v.trim()))
}

/// Header index and end (exclusive) of the first section named `section`.
pub(crate) fn find_section(lines: &[String], section: &str) -> Option<(usize, usize)> {
    let h = lines
        .iter()
        .position(|l| header_name(l).is_some_and(|n| n.eq_ignore_ascii_case(section)))?;
    let e = lines[h + 1..]
        .iter()
        .position(|l| header_name(l).is_some())
        .map_or(lines.len(), |p| h + 1 + p);
    Some((h, e))
}

/// Index of the first line in `range` that sets `key`.
pub(crate) fn find_key(
    lines: &[String],
    range: std::ops::Range<usize>,
    key: &str,
) -> Option<usize> {
    range
        .into_iter()
        .find(|&i| parse_kv(&lines[i]).is_some_and(|(k, _)| k.eq_ignore_ascii_case(key)))
}
