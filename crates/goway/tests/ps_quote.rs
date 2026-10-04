//! Audit 3, M2: `ps_quote` and `ps_call` must deliver any string to PowerShell exactly, with
//! no side effect. PowerShell reads U+2018 to U+201B as single quotes, so an unescaped
//! typographic quote ends the literal and the rest becomes code. These tests evaluate the
//! quoted text with a real PowerShell (`GOWAY_PWSH`, else `pwsh`, else `powershell` on
//! Windows) and pass trivially without one (the CI jobs have it).

use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;

use base64::Engine as _;
use goway::transport::{encoded_command, ps_call, ps_quote};
use proptest::prelude::*;

fn pwsh() -> Option<&'static PathBuf> {
    static PS: OnceLock<Option<PathBuf>> = OnceLock::new();
    PS.get_or_init(|| {
        let mut candidates = Vec::new();
        if let Some(p) = std::env::var_os("GOWAY_PWSH") {
            candidates.push(PathBuf::from(p));
        }
        candidates.push(PathBuf::from("pwsh"));
        if cfg!(windows) {
            candidates.push(PathBuf::from("powershell"));
        }
        candidates.into_iter().find(|c| {
            Command::new(c)
                .args(["-NoProfile", "-Command", "exit 0"])
                .output()
                .is_ok_and(|o| o.status.success())
        })
    })
    .as_ref()
}

/// Send every string through `& 'Show' <quoted>` and back as UTF-8 bytes; also report whether
/// any injected statement ran (`$global:hit`).
fn round_trip(pwsh: &PathBuf, strings: &[String]) -> (Vec<String>, bool) {
    let mut script = String::from(
        "$ErrorActionPreference = 'Stop'; \
         function Show { param($x) [Console]::Out.WriteLine([Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes([string]$x))) }; ",
    );
    for s in strings {
        script.push_str(&ps_call(&["Show", s]));
        script.push_str("; ");
    }
    script.push_str("[Console]::Out.WriteLine('hit=' + ($null -ne $global:hit))");
    let out = Command::new(pwsh)
        .args(["-NoProfile", "-NonInteractive", "-EncodedCommand"])
        .arg(encoded_command(&script))
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "pwsh failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8(out.stdout).unwrap();
    let mut lines: Vec<&str> = text.lines().collect();
    let hit = lines.pop() == Some("hit=True");
    let decoded = lines
        .iter()
        .map(|l| {
            String::from_utf8(
                base64::engine::general_purpose::STANDARD
                    .decode(l.trim())
                    .unwrap(),
            )
            .unwrap()
        })
        .collect();
    (decoded, hit)
}

/// The strings that matter: every quote-like and expansion-like character, alone and around an
/// injection attempt that would set `$global:hit` if it ever ran as code.
fn nasty() -> Vec<String> {
    let mut v: Vec<String> = [
        "'",
        "\u{2018}",
        "\u{2019}",
        "\u{201a}",
        "\u{201b}",
        "\"",
        "\u{201c}",
        "\u{201d}",
        "\u{201e}",
        "$",
        "$(1+1)",
        "`",
        "`n",
        ";",
        "&",
        "|",
        "\n",
        "\r\n",
        "",
        " ",
        "a b",
        "$env:PATH",
        "@'\nx\n'@",
        "{",
        "}",
        "(",
        ")",
        "#",
        "<#",
        "#>",
        "\\",
        "%",
    ]
    .iter()
    .map(|s| (*s).to_owned())
    .collect();
    for q in ["'", "\u{2018}", "\u{2019}", "\u{201a}", "\u{201b}"] {
        v.push(format!("x{q}; $global:hit = 1; {q}y"));
        v.push(format!("{q}{q}; $global:hit = 1; {q}{q}"));
        v.push(format!("{q}$(($global:hit = 1))"));
    }
    v
}

#[test]
fn quote_characters_and_injection_attempts_arrive_byte_exact_and_run_nothing() {
    let Some(pwsh) = pwsh() else { return };
    let strings = nasty();
    let (got, hit) = round_trip(pwsh, &strings);
    assert!(!hit, "an argument ran as PowerShell code");
    assert_eq!(got, strings);
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 8, ..ProptestConfig::default() })]

    /// Arbitrary Unicode strings, with the dangerous characters mixed in, round-trip exactly.
    #[test]
    fn any_string_round_trips_through_powershell(
        raw in proptest::collection::vec(
            proptest::string::string_regex(
                "[\\x{1}-\\x{7f}\\x{2018}-\\x{201f}\\x{a0}-\\x{2ff}\\x{4e00}-\\x{4e10}\\x{1f600}-\\x{1f60f}']{0,24}"
            ).unwrap(),
            1..24,
        ),
        free in proptest::collection::vec(any::<String>(), 0..8),
    ) {
        let Some(pwsh) = pwsh() else { return Ok(()) };
        let strings: Vec<String> = raw.into_iter().chain(free).map(|s| s.replace('\0', "")).collect();
        let (got, hit) = round_trip(pwsh, &strings);
        prop_assert!(!hit, "an argument ran as PowerShell code");
        prop_assert_eq!(got, strings);
    }
}

#[test]
fn the_quoted_form_is_one_single_quoted_literal() {
    for s in nasty() {
        let q = ps_quote(&s);
        assert!(q.starts_with('\'') && q.ends_with('\''), "{q}");
        // Every typographic single quote inside is doubled.
        let inner = &q[1..q.len() - 1];
        for c in ['\'', '\u{2018}', '\u{2019}', '\u{201a}', '\u{201b}'] {
            assert_eq!(inner.matches(c).count(), 2 * s.matches(c).count(), "{q}");
        }
    }
}
