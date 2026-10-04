//! `goway.toml`: project rules at the repository root.
//!
//! ```toml
//! [[rule]]
//! command = "cargo nextest*"   # glob over the command line, `*` and `?`
//! needs = ["mem>=8G"]
//! prefers = ["cpu=avx2"]
//! ```
//!
//! `[translate]` maps a program to its replacement per OS, for example
//! `mytool = { windows = "mytool.cmd", linux = "mytool" }`; targets are bare program
//! names (looked up on PATH directories only) or work-tree paths. The file is part of the
//! repository, so these entries have the repository's own trust level.
//!
//! A top-level `cross_os = true` lets hosts of every OS take the project's runs
//! (`false` keeps the laptop's OS and silences the cross-OS hint).
//!
//! The first rule whose glob matches the whole command line (words joined
//! by single spaces) supplies `needs` and `prefers` (the terms of
//! [`crate::needs`]). They are merged with the command line's own terms,
//! and the command line wins: a command-line term replaces a rule term of
//! the same key (`mem>=16G` replaces `mem>=8G`) in the same list. The file
//! has no place for host names or secrets (an unknown key is an error with
//! the file and line); it is an ordinary tracked file and is synced like
//! any other.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::needs::{Selection, Term};

/// The file's name at the repository root.
pub const FILE: &str = "goway.toml";

/// Largest `goway.toml` goway reads.
const MAX_BYTES: u64 = 64 * 1024;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFile {
    #[serde(default)]
    rule: Vec<RawRule>,
    #[serde(default)]
    toolchain: RawToolchain,
    /// Top-level `with_git = true`: every run gets a `.git` (see `--with-git`).
    #[serde(default)]
    with_git: bool,
    /// Top-level `cross_os`: `true` lets every OS take runs, `false` silences the hint.
    #[serde(default)]
    cross_os: Option<bool>,
    /// `[translate]`: per-program translations by OS (see [`crate::translate`]).
    #[serde(default)]
    translate: std::collections::BTreeMap<String, RawTranslate>,
}

/// One `[translate]` entry: the replacement program per OS.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawTranslate {
    windows: Option<String>,
    linux: Option<String>,
    macos: Option<String>,
}

/// `[toolchain]`: `tools = [...]`, `rust_targets`, `packages`, plus `tool = "version"` pins.
#[derive(Debug, Default, Deserialize)]
struct RawToolchain {
    #[serde(default)]
    tools: Vec<String>,
    #[serde(default)]
    rust_targets: Vec<String>,
    #[serde(default)]
    packages: RawPackages,
    #[serde(flatten)]
    versions: std::collections::BTreeMap<String, String>,
}

/// `[toolchain] packages`: names per package manager; any other manager is an error.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPackages {
    #[serde(default)]
    apt: Vec<String>,
    #[serde(default)]
    dnf: Vec<String>,
    #[serde(default)]
    pacman: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRule {
    command: String,
    #[serde(default)]
    needs: Vec<String>,
    #[serde(default)]
    prefers: Vec<String>,
}

/// One validated rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rule {
    /// Glob over the command line.
    pub command: String,
    /// Hard requirements.
    pub needs: Vec<Term>,
    /// Soft preferences.
    pub prefers: Vec<Term>,
}

/// The rules of one `goway.toml`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Rules {
    /// In file order; the first match applies.
    pub rules: Vec<Rule>,
    /// `[toolchain]`: the tools doctor checks and the versions it pins.
    pub toolchain: crate::doctor::Toolchain,
    /// `with_git = true`: every run of this project gets a `.git`.
    pub with_git: bool,
    /// `cross_os`: `Some(true)` allows every OS, `Some(false)` keeps the
    /// laptop's OS without the hint, `None` (unset) keeps it with the hint.
    pub cross_os: Option<bool>,
    /// `[translate]`: the project's program translations.
    pub translate: crate::translate::Overrides,
}

/// Whether the project's `goway.toml` asks for `with_git`.
///
/// # Errors
///
/// [`Rules::load`]'s errors.
pub fn wants_git(root: &Path) -> Result<bool> {
    Ok(Rules::load(root)?.is_some_and(|r| r.with_git))
}

/// The project's `[translate]` entries (none without a `goway.toml`).
///
/// # Errors
///
/// [`Rules::load`]'s errors.
pub fn translate_overrides(root: &Path) -> Result<crate::translate::Overrides> {
    Ok(Rules::load(root)?.map(|r| r.translate).unwrap_or_default())
}

/// The project's `cross_os` setting (`None` when unset or there is no `goway.toml`).
///
/// # Errors
///
/// [`Rules::load`]'s errors.
pub fn cross_os_setting(root: &Path) -> Result<Option<bool>> {
    Ok(Rules::load(root)?.and_then(|r| r.cross_os))
}

/// Configured hosts of another OS family than the laptop's (a Mac laptop treats `linux`
/// hosts as possible Macs, so only Windows hosts count there).
pub fn other_os_hosts<'a>(config: &'a crate::config::Config, laptop: &str) -> Vec<&'a str> {
    config
        .hosts
        .iter()
        .filter(|h| {
            let os = h.os.as_str();
            os != laptop && !(laptop == "darwin" && os == "linux")
        })
        .map(|h| h.name.as_str())
        .collect()
}

/// The loud cross-OS hint: a portable runner stays on the laptop's OS while hosts of another
/// OS could take it. Silent for unrecognized commands, `cross_os` set either way, an explicit
/// `os=` need, `--any-os`, and a pinned host (`pool_os` is `None` then).
pub fn warn_cross_os(
    renderer: crate::render::Renderer,
    config: &crate::config::Config,
    command: &[String],
    selection: &Selection,
    setting: Option<bool>,
    overrides: &crate::translate::Overrides,
) {
    if selection.pool_os.is_none() || setting.is_some() {
        return;
    }
    // One source of truth: a runner goway knows, or a program the translation table covers.
    let runner = crate::runners::portable_runner(command).or_else(|| {
        crate::translate::is_translatable(command, overrides)
            .then(|| command.first().map_or("command", String::as_str))
    });
    let Some(runner) = runner else {
        return;
    };
    let hosts = other_os_hosts(config, crate::needs::laptop_os());
    if hosts.is_empty() {
        return;
    }
    tracing::info!(runner, ?hosts, "cross-OS hint");
    let laptop = crate::needs::laptop_os();
    renderer.warn(format_args!(
        "CROSS-OS: `{runner}` is portable, but this run stays on {laptop} hosts"
    ));
    renderer.note(format_args!(
        "  other-OS hosts that could take it: {}",
        hosts.join(", ")
    ));
    renderer.note("  allow every OS for this run:      goway run --any-os -- ...");
    renderer.note(format_args!(
        "  allow it for this project:        cross_os = true in {FILE}"
    ));
    renderer.note(format_args!(
        "  silence this hint:                cross_os = false in {FILE}"
    ));
}

/// Which rule applied to a run, for the note and the report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Applied {
    /// 1-based position in `goway.toml`.
    pub rule: usize,
    /// The rule's command glob.
    pub command: String,
    /// The rule's needs, as written.
    pub needs: Vec<String>,
    /// The rule's preferences, as written.
    pub prefers: Vec<String>,
}

impl Applied {
    /// One line saying what applied, for the note.
    pub fn describe(&self) -> String {
        let terms = |label: &str, v: &[String]| {
            if v.is_empty() {
                String::new()
            } else {
                format!(" {label} {}", v.join(","))
            }
        };
        format!(
            "{FILE} rule {} (command = \"{}\") applies:{}{}",
            self.rule,
            self.command,
            terms("needs", &self.needs),
            terms("prefers", &self.prefers)
        )
    }
}

/// Whether `text` matches the whole of glob `pattern` (`*` any run of
/// characters, `?` one character; everything else literal).
pub fn glob_matches(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    let (mut pi, mut ti) = (0, 0);
    let (mut star, mut mark) = (None, 0);
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) && p[pi] != '*' {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

impl Rules {
    /// Parse `text` (read from `path`, which is only named in errors).
    ///
    /// # Errors
    ///
    /// [`Error::Config`] with the parser's file, line and column for a
    /// malformed file or unknown key, or naming the rule with a bad term.
    pub fn parse(text: &str, path: &Path) -> Result<Self> {
        let raw: RawFile = toml::from_str(text).map_err(|e| Error::Config {
            path: path.to_owned(),
            message: e.to_string(),
        })?;
        let mut rules = Vec::new();
        for (i, r) in raw.rule.into_iter().enumerate() {
            let terms = |list: &[String]| -> Result<Vec<Term>> {
                list.iter()
                    .map(|t| {
                        Term::parse(t).map_err(|e| Error::Config {
                            path: path.to_owned(),
                            message: format!("rule {} (command = \"{}\"): {e}", i + 1, r.command),
                        })
                    })
                    .collect()
            };
            if r.command.trim().is_empty() {
                return Err(Error::Config {
                    path: path.to_owned(),
                    message: format!("rule {} has an empty `command`", i + 1),
                });
            }
            rules.push(Rule {
                needs: terms(&r.needs)?,
                prefers: terms(&r.prefers)?,
                command: r.command,
            });
        }
        let mut translate = crate::translate::Overrides::default();
        for (program, entry) in raw.translate {
            let bad = |why: String| Error::Config {
                path: path.to_owned(),
                message: format!("[translate] {program}: {why}"),
            };
            if !crate::translate::safe_key(&program) {
                return Err(bad(
                    "the program name must not hold spaces, `;` or `,`".to_owned()
                ));
            }
            for (target, value) in [
                (crate::translate::Target::Windows, entry.windows),
                (crate::translate::Target::Linux, entry.linux),
                (crate::translate::Target::Macos, entry.macos),
            ] {
                if let Some(v) = value {
                    translate.set(
                        &program,
                        target,
                        crate::translate::dest_of(&v).map_err(bad)?,
                    );
                }
            }
        }
        let toolchain = crate::doctor::Toolchain {
            versions: raw.toolchain.versions,
            tools: raw.toolchain.tools,
            rust_targets: raw.toolchain.rust_targets,
            packages: crate::doctor::Packages {
                apt: raw.toolchain.packages.apt,
                dnf: raw.toolchain.packages.dnf,
                pacman: raw.toolchain.packages.pacman,
            },
        };
        Ok(Self {
            rules,
            toolchain,
            with_git: raw.with_git,
            cross_os: raw.cross_os,
            translate,
        })
    }

    /// Read `goway.toml` in `root`; `None` when there is none.
    ///
    /// # Errors
    ///
    /// [`Error::Io`] when it cannot be read or is too large, or [`Rules::parse`]'s errors.
    pub fn load(root: &Path) -> Result<Option<Self>> {
        let path = root.join(FILE);
        let meta = match std::fs::metadata(&path) {
            Ok(m) if m.is_file() => m,
            Ok(_) => return Ok(None),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(Error::io("read", &path, e)),
        };
        if meta.len() > MAX_BYTES {
            return Err(Error::Config {
                path,
                message: format!("larger than {MAX_BYTES} bytes"),
            });
        }
        let text = std::fs::read_to_string(&path).map_err(|e| Error::io("read", &path, e))?;
        let rules = Self::parse(&text, &path)?;
        tracing::debug!(rules = rules.rules.len(), "read {FILE}");
        Ok(Some(rules))
    }

    /// The first rule (and its 1-based number) matching `command`.
    pub fn first_match(&self, command: &[String]) -> Option<(usize, &Rule)> {
        let line = command.join(" ");
        self.rules
            .iter()
            .enumerate()
            .find(|(_, r)| glob_matches(&r.command, &line))
            .map(|(i, r)| (i + 1, r))
    }
}

/// Merge rule terms with command-line terms: the command line's terms stay,
/// and a rule term is dropped when the command line has one with the same key.
fn merged(rule: &[Term], cli: &[Term]) -> Vec<Term> {
    let mut out: Vec<Term> = rule
        .iter()
        .filter(|r| !cli.iter().any(|c| c.key() == r.key()))
        .cloned()
        .collect();
    out.extend(cli.iter().cloned());
    out
}

/// The selection of a run: the command line's `--needs`/`--prefers`
/// merged over the first matching rule of `goway.toml` in `root`.
///
/// # Errors
///
/// Bad command-line terms ([`Error::Usage`]) or a bad `goway.toml` ([`Error::Config`]).
pub fn selection_for(
    root: &Path,
    command: &[String],
    cli_needs: &[String],
    cli_prefers: &[String],
    any_os: bool,
) -> Result<(Selection, Option<Applied>)> {
    let rules = Rules::load(root)?;
    // Every OS is a candidate with --any-os or `cross_os = true`; else the laptop's family.
    let any = any_os || rules.as_ref().is_some_and(|r| r.cross_os == Some(true));
    let default_os = |s: Selection| {
        if any {
            s
        } else {
            s.with_default_os(crate::needs::laptop_os())
        }
    };
    let cli = default_os(Selection::parse(cli_needs, cli_prefers)?);
    let Some(rules) = rules else {
        return Ok((cli, None));
    };
    let Some((n, rule)) = rules.first_match(command) else {
        tracing::debug!("no {FILE} rule matches the command");
        return Ok((cli, None));
    };
    let applied = Applied {
        rule: n,
        command: rule.command.clone(),
        needs: rule.needs.iter().map(ToString::to_string).collect(),
        prefers: rule.prefers.iter().map(ToString::to_string).collect(),
    };
    tracing::info!(rule = n, "{FILE} rule applies");
    let selection = Selection {
        needs: merged(&rule.needs, &cli.needs),
        prefers: merged(&rule.prefers, &cli.prefers),
        pool_os: None,
        repo_id: None,
    };
    Ok((default_os(selection), Some(applied)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn parse(text: &str) -> Result<Rules> {
        Rules::parse(text, Path::new("goway.toml"))
    }

    fn words(s: &str) -> Vec<String> {
        s.split_whitespace().map(str::to_owned).collect()
    }

    // frob:ticket 01M42EZ3TAWHTCJ4MWVYKNEA39
    // frob:tests crates/goway/src/project.rs::selection_for
    #[test]
    fn a_run_defaults_to_the_laptops_os_unless_a_need_names_one() {
        let dir = tempfile::tempdir().unwrap();
        let none: Vec<String> = Vec::new();
        let (plain, _) = selection_for(dir.path(), &[], &none, &none, false).unwrap();
        assert_eq!(plain.pool_os.as_deref(), Some(crate::needs::laptop_os()));
        let (win, _) =
            selection_for(dir.path(), &[], &["os=windows".to_owned()], &none, false).unwrap();
        assert_eq!(win.pool_os, None);
    }

    // frob:ticket 01M42KTW3HF5XY7HAZD6P32EB5
    // frob:tests crates/goway/src/project.rs::Rules
    #[test]
    fn toolchain_reads_rust_targets_and_packages_and_refuses_unknown_managers() {
        let r = parse(
            "[toolchain]\nrust_targets = [\"x86_64-pc-windows-gnu\"]\ntools = [\"x86_64-w64-mingw32-gcc\"]\nsh = \">=1\"\n[toolchain.packages]\napt = [\"gcc-mingw-w64-x86-64\"]\ndnf = [\"mingw64-gcc\"]\n",
        )
        .unwrap();
        assert_eq!(r.toolchain.rust_targets, ["x86_64-pc-windows-gnu"]);
        assert_eq!(r.toolchain.packages.apt, ["gcc-mingw-w64-x86-64"]);
        assert_eq!(r.toolchain.packages.dnf, ["mingw64-gcc"]);
        assert!(r.toolchain.packages.pacman.is_empty());
        assert_eq!(r.toolchain.tools, ["x86_64-w64-mingw32-gcc"]);
        assert_eq!(
            r.toolchain.versions.get("sh").map(String::as_str),
            Some(">=1")
        );
        assert!(parse("[toolchain.packages]\nbrew = [\"x\"]\n").is_err());
    }

    // frob:tests crates/goway/src/project.rs::glob_matches
    #[test]
    fn globs_match_the_whole_command_line() {
        assert!(glob_matches("cargo nextest*", "cargo nextest run -p x"));
        assert!(glob_matches("cargo * run", "cargo nextest run"));
        assert!(glob_matches("pytest?", "pytest3"));
        assert!(!glob_matches("pytest", "pytest -x"));
        assert!(!glob_matches("cargo nextest", "cargo nextest run"));
        assert!(glob_matches("*", ""));
        assert!(glob_matches("a*b*c", "aXXbYYc"));
        assert!(!glob_matches("a*b*c", "aXXbYY"));
    }

    proptest! {
        // frob:tests crates/goway/src/project.rs::glob_matches
        #[test]
        fn a_literal_matches_itself_and_star_matches_anything(s in "[a-z0-9 =>-]{0,30}", t in "[a-z0-9 ]{0,30}") {
            prop_assert!(glob_matches(&s, &s));
            prop_assert!(glob_matches("*", &t));
            let (prefix, suffix) = (format!("{s}*"), format!("*{s}"));
            let (with_tail, with_head) = (format!("{s}{t}"), format!("{t}{s}"));
            prop_assert!(glob_matches(&prefix, &with_tail));
            prop_assert!(glob_matches(&suffix, &with_head));
            prop_assert_eq!(glob_matches(&s, &t), s == t);
        }
    }

    // frob:tests crates/goway/src/project.rs::selection_for
    #[test]
    fn the_first_matching_rule_applies_and_the_command_line_wins() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(FILE),
            r#"
[[rule]]
command = "cargo nextest*"
needs = ["mem>=8G", "arch=x86_64"]
prefers = ["cpu=avx2"]

[[rule]]
command = "cargo *"
needs = ["gpu"]
"#,
        )
        .unwrap();
        let strings = |v: &[&str]| v.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        // Rule 1 applies (not rule 2), and `mem>=16G` replaces the rule's `mem>=8G`.
        let (sel, applied) = selection_for(
            dir.path(),
            &words("cargo nextest run"),
            &strings(&["mem>=16G", "kvm"]),
            &[],
            false,
        )
        .unwrap();
        let applied = applied.unwrap();
        assert_eq!(applied.rule, 1);
        assert!(applied.describe().contains("rule 1 (command = \"cargo nextest*\") applies: needs mem>=8G,arch=x86_64 prefers cpu=avx2"), "{}", applied.describe());
        let needs: Vec<String> = sel.needs.iter().map(ToString::to_string).collect();
        assert_eq!(needs, ["arch=x86_64", "mem>=16G", "kvm"]);
        assert_eq!(sel.prefers.len(), 1);
        // Rule 2 for another cargo command; no rule for others.
        let (sel, applied) =
            selection_for(dir.path(), &words("cargo build"), &[], &[], false).unwrap();
        assert_eq!(applied.unwrap().rule, 2);
        assert_eq!(sel.needs.len(), 1);
        let (sel, applied) = selection_for(
            dir.path(),
            &words("make"),
            &strings(&["docker"]),
            &[],
            false,
        )
        .unwrap();
        assert!(applied.is_none());
        assert_eq!(sel.needs.len(), 1);
    }

    #[test]
    fn no_file_means_the_command_line_alone() {
        let dir = tempfile::tempdir().unwrap();
        let (sel, applied) =
            selection_for(dir.path(), &words("make"), &["kvm".to_owned()], &[], false).unwrap();
        assert!(applied.is_none());
        assert_eq!(sel.needs.len(), 1);
    }

    // frob:tests crates/goway/src/project.rs::Rules
    #[test]
    fn unknown_keys_and_bad_terms_are_errors_naming_file_and_line() {
        for (text, needle) in [
            ("[[rule]]\ncommand = \"x\"\nhost = \"helios\"\n", "line 3"),
            ("[[host]]\nname = \"helios\"\n", "line 1"),
            ("[defaults]\nport = 22\n", "line 1"),
            ("token = \"s3cret\"\n", "line 1"),
            (
                "[[rule]]\ncommand = \"x\"\nneeds = [\"gpus\"]\n",
                "unknown key `gpus`",
            ),
            ("[[rule]]\ncommand = \"\"\n", "empty `command`"),
            ("[[rule]]\nneeds = []\n", "command"),
        ] {
            let err = parse(text).unwrap_err().to_string();
            assert!(err.contains("goway.toml"), "{err}");
            assert!(err.contains(needle), "{text}: {err}");
        }
        assert!(parse("").unwrap().rules.is_empty());
    }

    // frob:tests crates/goway/src/project.rs::Rules
    #[test]
    fn an_oversized_file_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(FILE), vec![b'#'; 70_000]).unwrap();
        assert!(
            Rules::load(dir.path())
                .unwrap_err()
                .to_string()
                .contains("larger than")
        );
    }

    // frob:ticket 01M42FJVGY91ND091THEDPP8DN
    // frob:tests crates/goway/src/project.rs::other_os_hosts
    #[test]
    fn cross_os_parses_and_other_os_hosts_exclude_the_laptops_family() {
        assert_eq!(parse("cross_os = true\n").unwrap().cross_os, Some(true));
        assert_eq!(parse("cross_os = false\n").unwrap().cross_os, Some(false));
        assert_eq!(parse("").unwrap().cross_os, None);
        assert!(parse("cross_os = \"maybe\"\n").is_err());
        let config: crate::config::Config = toml::from_str(
            "[[host]]\nname = \"a\"\naddress = \"192.0.2.1\"\n[[host]]\nname = \"w\"\nos = \"windows\"\naddress = \"192.0.2.2\"\n",
        )
        .unwrap();
        assert_eq!(other_os_hosts(&config, "linux"), ["w"]);
        assert_eq!(other_os_hosts(&config, "windows"), ["a"]);
        assert_eq!(other_os_hosts(&config, "darwin"), ["w"]);
    }

    // frob:ticket 01M42FJVGY91ND091THEDPP8DN
    // frob:tests crates/goway/src/project.rs::selection_for
    #[test]
    fn any_os_or_cross_os_true_lifts_the_default_pool_os() {
        let dir = tempfile::tempdir().unwrap();
        let none: Vec<String> = Vec::new();
        let (any, _) = selection_for(dir.path(), &[], &none, &none, true).unwrap();
        assert_eq!(any.pool_os, None);
        std::fs::write(dir.path().join(FILE), "cross_os = true\n").unwrap();
        let (any, _) = selection_for(dir.path(), &[], &none, &none, false).unwrap();
        assert_eq!(any.pool_os, None);
        std::fs::write(dir.path().join(FILE), "cross_os = false\n").unwrap();
        let (same, _) = selection_for(dir.path(), &[], &none, &none, false).unwrap();
        assert!(same.pool_os.is_some());
    }
}
