//! The one PowerShell string quoter, shared by goway and goway-setup.

/// The characters PowerShell reads as a single quote: U+0027 and the four typographic ones
/// (U+2018 to U+201B). Any of them ends a single-quoted string, and a doubled one is a literal.
const SINGLE_QUOTES: [char; 5] = ['\'', '\u{2018}', '\u{2019}', '\u{201a}', '\u{201b}'];

/// Quote `value` as a PowerShell single-quoted literal: nothing in it is ever interpreted (no
/// `$(...)`, backtick, splitting or variable), and every quote character PowerShell recognizes
/// (see [`SINGLE_QUOTES`]) is doubled so it cannot end the literal.
pub fn ps_quote(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('\'');
    for c in value.chars() {
        out.push(c);
        if SINGLE_QUOTES.contains(&c) {
            out.push(c);
        }
    }
    out.push('\'');
    out
}

#[cfg(test)]
mod tests {
    use super::ps_quote;

    #[test]
    fn every_quote_character_powershell_knows_is_doubled() {
        assert_eq!(ps_quote("it's"), "'it''s'");
        for q in ['\u{2018}', '\u{2019}', '\u{201a}', '\u{201b}'] {
            assert_eq!(ps_quote(&format!("a{q}b")), format!("'a{q}{q}b'"));
        }
        assert_eq!(ps_quote(""), "''");
    }

    #[test]
    fn nothing_else_is_touched_so_expansion_stays_off() {
        let raw = "$(Get-Date) `n \"x\" $env:A;& \u{201c}z\u{201d}\n";
        assert_eq!(ps_quote(raw), format!("'{raw}'"));
    }
}
