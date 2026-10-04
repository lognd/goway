//! Portable command translation: `python3` becomes `py -3` on Windows, `./gradlew` becomes
//! `gradlew.bat`, `./build/x` becomes `build\x.exe`.
//!
//! Only argv[0] is ever looked at or changed, by exact match against one declarative
//! table (plus `[translate]` entries from `goway.toml`). This module plans: it turns a command
//! into a candidate list for the host's OS; the host (the `resolve` verb of `remote.sh` and
//! `remote.ps1`) decides, searching PATH directories only. Doubt means no translation: every
//! function here is total, and a malformed answer is [`Outcome::Doubt`]. See
//! docs/design.md ("Portable command translation").

use std::collections::BTreeMap;
use std::fmt::Write as _;

use serde::Serialize;

/// The OS a command is translated for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// A Windows host.
    Windows,
    /// A Linux host (or WSL).
    Linux,
    /// A macOS host.
    Macos,
}

impl Target {
    /// The target for a host that reports `os` (`windows`, `darwin`, anything else is Linux).
    pub fn of(os: &str) -> Self {
        match os.to_ascii_lowercase().as_str() {
            "windows" => Self::Windows,
            "darwin" | "macos" => Self::Macos,
            _ => Self::Linux,
        }
    }

    /// The `[translate]` key naming this OS.
    pub fn key(self) -> &'static str {
        match self {
            Self::Windows => "windows",
            Self::Linux => "linux",
            Self::Macos => "macos",
        }
    }
}

/// Where a candidate is looked for on the host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// The program as given, looked up on PATH (a hit means "leave the command alone").
    Same,
    /// A bare program name, looked up on PATH directories only.
    Bare,
    /// A path inside the synced work tree.
    Tree,
}

impl Kind {
    fn word(self) -> &'static str {
        match self {
            Self::Same => "same",
            Self::Bare => "bare",
            Self::Tree => "tree",
        }
    }
}

/// One built-in candidate: where to look, the fixed leading arguments, and an optional
/// self-check (arguments the program must run with and exit 0 within seconds).
struct Cand {
    kind: Kind,
    name: &'static str,
    args: &'static [&'static str],
    check: &'static [&'static str],
}

/// One row of the table: a program and its ordered candidates per OS family.
struct Row {
    program: &'static str,
    windows: &'static [Cand],
    unix: &'static [Cand],
}

const SAME: Cand = Cand {
    kind: Kind::Same,
    name: "",
    args: &[],
    check: &[],
};

const PY3: Cand = Cand {
    kind: Kind::Bare,
    name: "py.exe",
    args: &["-3"],
    check: &["-3", "-c", "pass"],
};

const PYTHON_EXE: Cand = Cand {
    kind: Kind::Bare,
    name: "python.exe",
    args: &[],
    check: &["-c", "pass"],
};

const PY3_PIP: Cand = Cand {
    kind: Kind::Bare,
    name: "py.exe",
    args: &["-3", "-m", "pip"],
    check: &["-3", "-m", "pip", "--version"],
};

const PYTHON3: Cand = Cand {
    kind: Kind::Bare,
    name: "python3",
    args: &[],
    check: &[],
};

const PIP3: Cand = Cand {
    kind: Kind::Bare,
    name: "pip3",
    args: &[],
    check: &[],
};

/// The built-in table: small, reviewed, one entry per program. `node`, `npm`, `npx`, `cargo`,
/// `go` and `dotnet` need none (the same name works everywhere).
const TABLE: &[Row] = &[
    Row {
        program: "python",
        windows: &[PY3, PYTHON_EXE],
        unix: &[SAME, PYTHON3],
    },
    Row {
        program: "python3",
        windows: &[PY3, PYTHON_EXE],
        unix: &[SAME],
    },
    Row {
        program: "pip",
        windows: &[PY3_PIP],
        unix: &[SAME, PIP3],
    },
    Row {
        program: "pip3",
        windows: &[PY3_PIP],
        unix: &[SAME],
    },
];

/// One candidate as sent to the host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    /// Candidates with a lower tier are preferred; two hits in one tier are doubt.
    pub tier: u8,
    kind: Kind,
    /// The program name (bare), the path inside the work tree (`/` separators), or the
    /// original argv[0] (same).
    pub name: String,
    /// Fixed leading arguments the translation adds.
    pub args: Vec<String>,
    /// Arguments of the self-check, empty for none.
    pub check: Vec<String>,
}

/// A `[translate]` target of `goway.toml`: a bare program name or a work-tree path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Dest {
    /// A bare program name, resolved from PATH directories.
    Bare(String),
    /// A path inside the work tree (`./dir/tool`, `dir\tool.cmd`).
    Tree(String),
}

/// Per-program `[translate]` overrides, by OS.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Overrides {
    entries: BTreeMap<String, BTreeMap<&'static str, Dest>>,
}

impl Overrides {
    /// Add (or replace) the entry for `program` on `target`.
    pub fn set(&mut self, program: &str, target: Target, dest: Dest) {
        self.entries
            .entry(program.to_owned())
            .or_default()
            .insert(target.key(), dest);
    }

    /// Whether there are no entries.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// A `[translate]` value: a bare program name, or a work-tree path (it has a `/` or `\`).
///
/// # Errors
///
/// Why the value is not one: an absolute path, `..`, odd characters, or an empty value.
pub fn dest_of(value: &str) -> Result<Dest, String> {
    if value.contains(['/', '\\']) {
        safe_tree_path(value).map(Dest::Tree).ok_or_else(|| {
            format!("`{value}` is not a path inside the work tree (no `..`, no absolute paths)")
        })
    } else if safe_bare(value) {
        Ok(Dest::Bare(value.to_owned()))
    } else {
        Err(format!(
            "`{value}` is not a bare program name (letters, digits and ._+- only)"
        ))
    }
}

/// Whether `key` may name a program in `[translate]`.
pub fn safe_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= 200
        && !key
            .bytes()
            .any(|b| b.is_ascii_control() || b == b' ' || b == b';' || b == b',')
}

/// What the client asks the host to resolve.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// The program as the user wrote it.
    pub requested: String,
    /// The candidates, in preference order.
    pub candidates: Vec<Candidate>,
}

impl Plan {
    /// The `resolve` verb's stdin: one `tier;kind;name;args;check` line per candidate
    /// (arguments comma-joined). Every field is validated to hold neither `;` nor `,`.
    pub fn spec(&self) -> String {
        let mut out = String::new();
        for c in &self.candidates {
            let name = if c.kind == Kind::Same {
                self.requested.as_str()
            } else {
                c.name.as_str()
            };
            let _ = writeln!(
                out,
                "{};{};{};{};{}",
                c.tier,
                c.kind.word(),
                name,
                c.args.join(","),
                c.check.join(",")
            );
        }
        out
    }
}

/// A bare program name safe to send and to run: letters, digits and `._+-`, no path part.
fn safe_bare(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._+-".contains(&b))
        && name != "."
        && name != ".."
}

/// A path inside the work tree: relative, no `..`, no empty or `.` segments after the
/// leading `./`, and only characters that cannot break the spec or a shell.
fn safe_tree_path(path: &str) -> Option<String> {
    let rel = path.strip_prefix("./").unwrap_or(path).replace('\\', "/");
    if rel.is_empty() || rel.starts_with('/') || rel.len() > 200 {
        return None;
    }
    let ok = rel.split('/').all(|seg| {
        !seg.is_empty()
            && seg != "."
            && seg != ".."
            && seg
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._+- ".contains(&b))
    });
    ok.then_some(rel)
}

/// The work-tree path a relative argv[0] (`./x/y`) names, if it is one.
fn tree_program(argv0: &str) -> Option<String> {
    if !argv0.starts_with("./") || argv0.contains('\\') || argv0.contains(' ') {
        return None;
    }
    safe_tree_path(argv0)
}

fn cand(tier: u8, kind: Kind, name: &str, args: &[&str], check: &[&str]) -> Candidate {
    Candidate {
        tier,
        kind,
        name: name.to_owned(),
        args: args.iter().map(|s| (*s).to_owned()).collect(),
        check: check.iter().map(|s| (*s).to_owned()).collect(),
    }
}

fn builtin(row_cands: &[Cand]) -> Vec<Candidate> {
    row_cands
        .iter()
        .enumerate()
        .map(|(i, c)| {
            cand(
                u8::try_from(i).unwrap_or(u8::MAX),
                c.kind,
                c.name,
                c.args,
                c.check,
            )
        })
        .collect()
}

/// Candidates for a work-tree program on Windows: `gradlew` and `mvnw` have fixed wrappers;
/// any other extension-less program is its `.exe`, else the multi-config build directories.
fn windows_tree(rel: &str) -> Vec<Candidate> {
    let (dir, file) = rel.rsplit_once('/').map_or(("", rel), |(d, f)| (d, f));
    let join = |sub: &str, name: &str| {
        [dir, sub, name]
            .iter()
            .filter(|s| !s.is_empty())
            .copied()
            .collect::<Vec<_>>()
            .join("/")
    };
    match file {
        "gradlew" => vec![cand(0, Kind::Tree, &join("", "gradlew.bat"), &[], &[])],
        "mvnw" => vec![cand(0, Kind::Tree, &join("", "mvnw.cmd"), &[], &[])],
        f if !f.contains('.') => vec![
            cand(0, Kind::Tree, &join("", &format!("{f}.exe")), &[], &[]),
            cand(1, Kind::Tree, &join("Debug", &format!("{f}.exe")), &[], &[]),
            cand(
                1,
                Kind::Tree,
                &join("Release", &format!("{f}.exe")),
                &[],
                &[],
            ),
        ],
        _ => Vec::new(),
    }
}

fn from_dest(argv0: &str, dest: &Dest) -> Vec<Candidate> {
    match dest {
        Dest::Bare(name) if name == argv0 => vec![cand(0, Kind::Same, "", &[], &[])],
        Dest::Bare(name) if safe_bare(name) => vec![cand(0, Kind::Bare, name, &[], &[])],
        Dest::Tree(path) => safe_tree_path(path)
            .map(|p| vec![cand(0, Kind::Tree, &p, &[], &[])])
            .unwrap_or_default(),
        Dest::Bare(_) => Vec::new(),
    }
}

/// The candidates for `command`'s argv[0] on `target`, or `None` when nothing is to be
/// resolved: the program has no entry, or every candidate is "the name as given".
pub fn plan(command: &[String], target: Target, overrides: &Overrides) -> Option<Plan> {
    let argv0 = command.first()?;
    let candidates = if let Some(entry) = overrides.entries.get(argv0) {
        entry
            .get(target.key())
            .map(|d| from_dest(argv0, d))
            .unwrap_or_default()
    } else if let Some(row) = TABLE.iter().find(|r| r.program == argv0) {
        builtin(if target == Target::Windows {
            row.windows
        } else {
            row.unix
        })
    } else if target == Target::Windows {
        tree_program(argv0)
            .map(|p| windows_tree(&p))
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    // Nothing to ask when the only way out is the program as given.
    if candidates.is_empty() || candidates.iter().all(|c| c.kind == Kind::Same) {
        return None;
    }
    Some(Plan {
        requested: argv0.clone(),
        candidates,
    })
}

/// Whether goway knows a translation for `command`'s argv[0] on some OS (so the cross-OS
/// default and hint treat it as cross-platform).
pub fn is_translatable(command: &[String], overrides: &Overrides) -> bool {
    let Some(argv0) = command.first() else {
        return false;
    };
    if overrides.entries.contains_key(argv0) || TABLE.iter().any(|r| r.program == argv0) {
        return true;
    }
    tree_program(argv0).is_some_and(|p| !windows_tree(&p).is_empty())
}

/// What the host answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The program as given is there: run the command unchanged.
    Unchanged,
    /// Run this program (an absolute path, or a work-tree path) with these leading arguments.
    Translated {
        /// The program to run.
        program: String,
        /// Fixed arguments that go before the command's own.
        args: Vec<String>,
    },
    /// Not certain: do not run a guess.
    Doubt(String),
}

/// Parse the host's one-line answer (`same`, `ok;PROGRAM;ARGS`, `none;WHY`); anything else,
/// including an unsafe program name, is doubt.
pub fn parse_answer(text: &str) -> Outcome {
    let line = text.lines().next().unwrap_or("").trim_end();
    let mut parts = line.split(';');
    match (parts.next(), parts.next(), parts.next(), parts.next()) {
        (Some("same"), None, ..) => Outcome::Unchanged,
        (Some("ok"), Some(program), args, None)
            if !program.is_empty() && !program.contains(['\n', '\r', '\0', ',']) =>
        {
            let args = args
                .unwrap_or("")
                .split(',')
                .filter(|a| !a.is_empty())
                .map(str::to_owned)
                .collect();
            Outcome::Translated {
                program: program.to_owned(),
                args,
            }
        }
        (Some("none"), why, ..) => Outcome::Doubt(why.unwrap_or("nothing found").to_owned()),
        _ => Outcome::Doubt("the host's answer was not understood".to_owned()),
    }
}

/// The command to run after a translation: only argv[0] is replaced (by `program` and its
/// fixed leading arguments); every other word is carried over byte for byte.
pub fn apply(command: &[String], program: &str, args: &[String]) -> Vec<String> {
    let mut out = Vec::with_capacity(command.len() + args.len());
    out.push(program.to_owned());
    out.extend(args.iter().cloned());
    out.extend(command.iter().skip(1).cloned());
    out
}

/// What was requested and what ran, for `--report` and the one-line note.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Translation {
    /// The program as the user wrote it.
    pub requested: String,
    /// The program that ran (an absolute path where the host resolved one).
    pub actual: String,
    /// The fixed arguments added before the command's own.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
}

impl Translation {
    /// `python3 -> py -3` for the note.
    pub fn describe(&self) -> String {
        let mut shown = std::path::Path::new(&self.actual.replace('\\', "/"))
            .file_name()
            .map_or_else(|| self.actual.clone(), |n| n.to_string_lossy().into_owned());
        for a in &self.args {
            shown.push(' ');
            shown.push_str(a);
        }
        format!("{} -> {shown}", self.requested)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn words(s: &str) -> Vec<String> {
        s.split_whitespace().map(str::to_owned).collect()
    }

    fn names(p: &Plan) -> Vec<String> {
        p.candidates
            .iter()
            .map(|c| {
                format!(
                    "{}:{}{}",
                    c.tier,
                    c.name,
                    if c.args.is_empty() {
                        String::new()
                    } else {
                        format!(" {}", c.args.join(" "))
                    }
                )
            })
            .collect()
    }

    // frob:ticket 01M42FN011Z3NBG491XAHHZCF5
    // frob:tests crates/goway/src/translate.rs::plan
    #[test]
    fn python_and_pip_translate_per_os_and_everything_else_does_not() {
        let none = Overrides::default();
        let win = |c: &str| plan(&words(c), Target::Windows, &none).map(|p| names(&p));
        assert_eq!(
            win("python3 -m pytest"),
            Some(vec!["0:py.exe -3".into(), "1:python.exe".into()])
        );
        assert_eq!(win("python x.py"), win("python3 x.py"));
        assert_eq!(
            win("pip install x"),
            Some(vec!["0:py.exe -3 -m pip".into()])
        );
        assert_eq!(win("pip3 list"), win("pip install x"));
        for same in [
            "node x.js",
            "npm test",
            "npx vitest",
            "cargo test",
            "go test",
            "dotnet test",
            "bash -c x",
            "PYTHON3",
        ] {
            assert_eq!(win(same), None, "{same}");
        }
        let lin = |c: &str, t| plan(&words(c), t, &none).map(|p| names(&p));
        // Linux: `python` may become python3 (the host decides); python3 and pip3 never change.
        assert_eq!(
            lin("python a.py", Target::Linux),
            Some(vec!["0:".into(), "1:python3".into()])
        );
        assert_eq!(
            lin("pip install", Target::Macos),
            Some(vec!["0:".into(), "1:pip3".into()])
        );
        assert_eq!(lin("python3 a.py", Target::Linux), None);
        assert_eq!(lin("pip3 list", Target::Linux), None);
    }

    // frob:ticket 01M42FN011Z3NBG491XAHHZCF5
    // frob:tests crates/goway/src/translate.rs::plan
    #[test]
    fn work_tree_programs_translate_on_windows_only_and_only_when_safe() {
        let none = Overrides::default();
        let win = |c: &str| plan(&words(c), Target::Windows, &none).map(|p| names(&p));
        assert_eq!(win("./gradlew test"), Some(vec!["0:gradlew.bat".into()]));
        assert_eq!(
            win("./sub/mvnw verify"),
            Some(vec!["0:sub/mvnw.cmd".into()])
        );
        assert_eq!(
            win("./build/app --x"),
            Some(vec![
                "0:build/app.exe".into(),
                "1:build/Debug/app.exe".into(),
                "1:build/Release/app.exe".into()
            ])
        );
        for no in [
            "./run.sh", "./a/../b", "./", "../x", "/abs/x", "x/y", "./a\\b", ".//x", "./a/./b",
        ] {
            assert_eq!(win(no), None, "{no}");
        }
        assert_eq!(plan(&words("./gradlew test"), Target::Linux, &none), None);
    }

    // frob:ticket 01M42FN011Z3NBG491XAHHZCF5
    // frob:tests crates/goway/src/translate.rs::plan
    #[test]
    fn project_entries_replace_the_built_in_ones_and_only_bare_names_or_tree_paths_pass() {
        let mut o = Overrides::default();
        o.set(
            "python3",
            Target::Windows,
            Dest::Bare("mypy3.exe".to_owned()),
        );
        o.set(
            "mytool",
            Target::Windows,
            Dest::Bare("mytool.cmd".to_owned()),
        );
        o.set("mytool", Target::Linux, Dest::Bare("mytool".to_owned()));
        o.set(
            "evil",
            Target::Windows,
            Dest::Bare("C:\\x\\evil.exe".to_owned()),
        );
        o.set("up", Target::Windows, Dest::Tree("../x".to_owned()));
        let win = |c: &str| plan(&words(c), Target::Windows, &o).map(|p| names(&p));
        assert_eq!(win("python3 x"), Some(vec!["0:mypy3.exe".into()]));
        assert_eq!(win("mytool a"), Some(vec!["0:mytool.cmd".into()]));
        assert_eq!(win("evil"), None, "a path is not a bare name");
        assert_eq!(win("up"), None, "no leaving the work tree");
        // An entry without this OS means no translation, not the built-in one.
        assert_eq!(plan(&words("mytool a"), Target::Macos, &o), None);
        assert_eq!(
            plan(&words("mytool a"), Target::Linux, &o),
            None,
            "same name: unchanged"
        );
        assert!(is_translatable(&words("mytool"), &o) && is_translatable(&words("python"), &o));
        assert!(!is_translatable(&words("cargo test"), &o));
    }

    // frob:ticket 01M42FN011Z3NBG491XAHHZCF5
    // frob:tests crates/goway/src/translate.rs::parse_answer
    #[test]
    fn answers_are_parsed_strictly_and_anything_odd_is_doubt() {
        assert_eq!(parse_answer("same\n"), Outcome::Unchanged);
        assert_eq!(
            parse_answer("ok;C:\\Windows\\py.exe;-3\n"),
            Outcome::Translated {
                program: "C:\\Windows\\py.exe".into(),
                args: vec!["-3".into()]
            }
        );
        assert_eq!(
            parse_answer("ok;/usr/bin/python3;"),
            Outcome::Translated {
                program: "/usr/bin/python3".into(),
                args: vec![]
            }
        );
        for bad in [
            "", "ok", "ok;", "ok;a;b;c", "same;x", "ok;a,b;", "maybe;x", "ok;a\rb;",
        ] {
            assert!(matches!(parse_answer(bad), Outcome::Doubt(_)), "{bad:?}");
        }
        assert_eq!(
            parse_answer("none;ambiguous"),
            Outcome::Doubt("ambiguous".into())
        );
    }

    // frob:ticket 01M42FN011Z3NBG491XAHHZCF5
    // frob:tests crates/goway/src/translate.rs::Plan
    #[test]
    fn the_spec_is_one_validated_line_per_candidate() {
        let p = plan(&words("python3 x"), Target::Windows, &Overrides::default()).unwrap();
        assert_eq!(
            p.spec(),
            "0;bare;py.exe;-3;-3,-c,pass\n1;bare;python.exe;;-c,pass\n"
        );
        let p = plan(&words("python a"), Target::Linux, &Overrides::default()).unwrap();
        assert_eq!(p.spec(), "0;same;python;;\n1;bare;python3;;\n");
        let t = Translation {
            requested: "python3".into(),
            actual: "C:\\Windows\\py.exe".into(),
            args: vec!["-3".into()],
        };
        assert_eq!(t.describe(), "python3 -> py.exe -3");
    }

    proptest! {
        // frob:ticket 01M42FN011Z3NBG491XAHHZCF5
        // frob:tests crates/goway/src/translate.rs::apply
        #[test]
        fn only_argv0_ever_changes(
            command in proptest::collection::vec(".{0,12}", 1..8),
            program in "[A-Za-z0-9./:\\\\_-]{1,20}",
            args in proptest::collection::vec("-[a-z0-9]{0,3}", 0..3),
        ) {
            let out = apply(&command, &program, &args);
            prop_assert_eq!(&out[0], &program);
            prop_assert_eq!(&out[1..=args.len()], &args[..]);
            prop_assert_eq!(&out[1 + args.len()..], &command[1..]);
        }

        // frob:ticket 01M42FN011Z3NBG491XAHHZCF5
        // frob:tests crates/goway/src/translate.rs::plan
        #[test]
        fn a_plan_is_absent_or_made_only_of_safe_candidates(
            command in proptest::collection::vec(".{0,16}", 0..5),
            target in prop_oneof![Just(Target::Windows), Just(Target::Linux), Just(Target::Macos)],
        ) {
            if let Some(p) = plan(&command, target, &Overrides::default()) {
                prop_assert_eq!(&p.requested, &command[0]);
                let spec = p.spec();
                for line in spec.lines() {
                    prop_assert_eq!(line.split(';').count(), 5, "{}", line);
                }
                for c in &p.candidates {
                    prop_assert!(!c.name.contains("..") && !c.name.starts_with('/'));
                }
            }
        }

        // frob:ticket 01M42FN011Z3NBG491XAHHZCF5
        // frob:tests crates/goway/src/translate.rs::parse_answer
        #[test]
        fn parsing_an_answer_never_panics_and_translations_are_clean(text in ".{0,60}") {
            if let Outcome::Translated { program, args } = parse_answer(&text) {
                prop_assert!(!program.is_empty() && !program.contains([',', '\n', '\r', '\0']));
                prop_assert!(args.iter().all(|a| !a.is_empty() && !a.contains(',')));
            }
        }
    }
}
