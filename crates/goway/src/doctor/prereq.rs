//! Per-repository prerequisites: Rust targets and distro packages a
//! repository asks for in `goway.toml` `[toolchain]` (`rust_targets`,
//! `packages`) and in `rust-toolchain.toml` (`targets`).
//!
//! Doctor checks them on every Unix helper with one extra probe appended to
//! its `doctor` call. Rust targets install user-level (`rustup target add`
//! for the toolchain the project pins). Packages are root fixes: they go
//! through `--rsudo` and its confirmation, which names the repository and
//! `goway.toml` as their source, and every name is validated against the
//! package manager's name syntax first, so a repository can ask for package
//! names and nothing else (no options, paths or shell characters).

use std::collections::BTreeMap;
use std::fmt::Write as _;

use super::{Check, Fix, Level};
use crate::error::{Error, Result};
use crate::ssh;

/// The package managers goway installs for, and their packages per manager.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Packages {
    /// Debian and Ubuntu (`apt-get`).
    pub apt: Vec<String>,
    /// Fedora and Red Hat (`dnf`).
    pub dnf: Vec<String>,
    /// Arch (`pacman`).
    pub pacman: Vec<String>,
}

/// A manager word, its package list and its name syntax.
type Rule<'a> = (&'static str, &'a [String], fn(&str) -> bool);

impl Packages {
    fn is_empty(&self) -> bool {
        self.apt.is_empty() && self.dnf.is_empty() && self.pacman.is_empty()
    }

    /// The list for a manager word (`apt`, `dnf`, `pacman`).
    fn of(&self, manager: &str) -> &[String] {
        match manager {
            "apt" => &self.apt,
            "dnf" => &self.dnf,
            "pacman" => &self.pacman,
            _ => &[],
        }
    }

    /// The first name that is not valid for its manager, with the manager.
    fn invalid(&self) -> Option<(&'static str, &str)> {
        let all: [Rule<'_>; 3] = [
            ("apt", &self.apt, valid_apt),
            ("dnf", &self.dnf, valid_dnf),
            ("pacman", &self.pacman, valid_pacman),
        ];
        all.iter()
            .find_map(|(m, list, ok)| list.iter().find(|n| !ok(n)).map(|n| (*m, n.as_str())))
    }
}

/// What a repository asks of a host beyond tools: targets, packages, and who asked.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Prereqs {
    /// Rust target triples to have installed for the pinned toolchain.
    pub rust_targets: Vec<String>,
    /// The toolchain `rust-toolchain.toml` pins (`channel`), when it names one.
    pub channel: Option<String>,
    /// Distro packages, per package manager.
    pub packages: Packages,
    /// The repository's name, shown wherever its packages are offered.
    pub repo: String,
}

fn within(text: &str, max: usize, first: fn(u8) -> bool, rest: fn(u8) -> bool) -> bool {
    let b = text.as_bytes();
    !b.is_empty() && b.len() <= max && first(b[0]) && b.iter().all(|c| rest(*c))
}

/// Whether `name` is a Rust target triple (lower-case words, digits, `_`, `.`, `-`).
pub fn valid_target(name: &str) -> bool {
    within(
        name,
        64,
        |c| c.is_ascii_lowercase() || c.is_ascii_digit(),
        |c| c.is_ascii_lowercase() || c.is_ascii_digit() || b"_.-".contains(&c),
    )
}

/// Whether `name` is a rustup toolchain name (`stable`, `1.98.0`, `nightly-2026-01-01`).
pub fn valid_channel(name: &str) -> bool {
    within(
        name,
        64,
        |c| c.is_ascii_alphanumeric(),
        |c| c.is_ascii_alphanumeric() || b"._-".contains(&c),
    )
}

/// Debian policy: 2+ characters of lower-case letters, digits, `+`, `-`, `.`, starting alphanumeric.
fn valid_apt(name: &str) -> bool {
    name.len() >= 2
        && within(
            name,
            100,
            |c| c.is_ascii_lowercase() || c.is_ascii_digit(),
            |c| c.is_ascii_lowercase() || c.is_ascii_digit() || b"+-.".contains(&c),
        )
}

/// RPM names: letters, digits and `+ . _ -`, starting alphanumeric.
fn valid_dnf(name: &str) -> bool {
    within(
        name,
        100,
        |c| c.is_ascii_alphanumeric(),
        |c| c.is_ascii_alphanumeric() || b"+._-".contains(&c),
    )
}

/// Arch names: letters, digits and `@ . _ + -`, not starting with `-` or `.`.
fn valid_pacman(name: &str) -> bool {
    within(
        name,
        100,
        |c| c.is_ascii_alphanumeric() || b"@_+".contains(&c),
        |c| c.is_ascii_alphanumeric() || b"@._+-".contains(&c),
    )
}

impl Prereqs {
    /// Whether the repository asks for nothing here.
    pub fn is_empty(&self) -> bool {
        self.rust_targets.is_empty() && self.packages.is_empty()
    }

    /// Reject any name that is not strictly a name.
    ///
    /// # Errors
    ///
    /// [`Error::Config`] naming `goway.toml` and the offending entry.
    pub fn validate(&self, goway_toml: &std::path::Path) -> Result<()> {
        let bad = |message: String| Error::Config {
            path: goway_toml.to_owned(),
            message,
        };
        if let Some(t) = self.rust_targets.iter().find(|t| !valid_target(t)) {
            return Err(bad(format!(
                "[toolchain] rust_targets: `{t}` is not a Rust target triple"
            )));
        }
        if let Some(c) = self.channel.as_deref().filter(|c| !valid_channel(c)) {
            return Err(bad(format!(
                "toolchain channel `{c}` is not a toolchain name"
            )));
        }
        if let Some((manager, name)) = self.packages.invalid() {
            return Err(bad(format!(
                "[toolchain] packages.{manager}: `{name}` is not a {manager} package name \
                 (letters, digits and a few of `+ - . _`; no options, paths or spaces)"
            )));
        }
        Ok(())
    }

    /// The shell script that reports the installed targets and packages as
    /// `rt.*` and `pkg.*` facts; `None` when there is nothing to ask. Every
    /// name in it passed [`Prereqs::validate`], so single quotes hold them.
    pub fn probe_script(&self) -> Option<String> {
        if self.is_empty() {
            return None;
        }
        let mut s =
            String::from("PATH=\"${CARGO_HOME:-$HOME/.cargo}/bin:$HOME/.local/bin:$PATH\"\n");
        if !self.rust_targets.is_empty() {
            let tc = self
                .channel
                .as_deref()
                .map_or_else(String::new, |c| format!(" --toolchain '{c}'"));
            let _ = write!(
                s,
                "if command -v rustup >/dev/null 2>&1; then\n\
                 if l=$(rustup target list --installed{tc} 2>/dev/null); then\n\
                 printf 'rt.status=ok\\nrt.installed=%s\\n' \"$(printf '%s' \"$l\" | tr '\\n' ' ')\"\n\
                 else printf 'rt.status=no-toolchain\\n'; fi\n\
                 else printf 'rt.status=no-rustup\\n'; fi\n"
            );
        }
        if !self.packages.is_empty() {
            s.push_str(
                "if command -v apt-get >/dev/null 2>&1; then m=apt\n\
                 elif command -v dnf >/dev/null 2>&1; then m=dnf\n\
                 elif command -v pacman >/dev/null 2>&1; then m=pacman\n\
                 else m=none; fi\n\
                 printf 'pkg.mgr=%s\\n' \"$m\"\n\
                 case \"$m\" in\n",
            );
            for (m, test) in [
                (
                    "apt",
                    "dpkg-query -W -f='${Status}' \"$p\" 2>/dev/null | grep -q 'install ok installed'",
                ),
                ("dnf", "rpm -q \"$p\" >/dev/null 2>&1"),
                ("pacman", "pacman -Qi \"$p\" >/dev/null 2>&1"),
            ] {
                let names = self.packages.of(m).join(" ");
                let _ = write!(
                    s,
                    "{m}) for p in {names}; do if {test}; then printf 'pkg.%s=yes\\n' \"$p\"; else printf 'pkg.%s=no\\n' \"$p\"; fi; done ;;"
                );
                s.push('\n');
            }
            s.push_str("esac\n");
        }
        Some(s)
    }

    /// `base` (the `doctor` call) followed by this repository's probe, if it has one.
    pub fn wrap(&self, base: &str) -> String {
        match self.probe_script() {
            Some(script) => format!("{base}; bash -c {}", ssh::shell_quote(&script)),
            None => base.to_owned(),
        }
    }

    /// Checks (with fixes) for the targets and packages, from the probe's facts.
    /// A host that did not answer the probe (a Windows host) gets none.
    pub fn checks(&self, facts: &BTreeMap<String, String>) -> Vec<Check> {
        let mut out = Vec::new();
        if !self.rust_targets.is_empty() {
            self.target_checks(facts, &mut out);
        }
        if !self.packages.is_empty() {
            self.package_checks(facts, &mut out);
        }
        out
    }

    fn target_checks(&self, facts: &BTreeMap<String, String>, out: &mut Vec<Check>) {
        let toolchain = self.channel.as_deref().map_or_else(
            || "the default toolchain".to_owned(),
            |c| format!("toolchain {c}"),
        );
        let installed: Vec<&str> = facts
            .get("rt.installed")
            .map(|l| l.split_whitespace().collect())
            .unwrap_or_default();
        let note = |detail: String| Check {
            name: "rust targets".to_owned(),
            explain: None,
            level: Level::Warn,
            detail,
            fix: None,
        };
        match facts.get("rt.status").map(String::as_str) {
            None => {}
            Some("no-rustup") => out.push(note(
                "rustup is not installed, so Rust targets cannot be checked; fix the cargo check first"
                    .to_owned(),
            )),
            Some("ok") => {
                for t in &self.rust_targets {
                    out.push(self.target_check(t, &toolchain, installed.contains(&t.as_str())));
                }
            }
            Some(_) => out.push(note(format!(
                "{toolchain} is not installed for this user; fix the cargo check first"
            ))),
        }
    }

    fn target_check(&self, target: &str, toolchain: &str, present: bool) -> Check {
        let toolchain_flag = self
            .channel
            .as_deref()
            .map_or_else(String::new, |c| format!(" --toolchain {c}"));
        Check {
            name: format!("target:{target}"),
            explain: Some(format!(
                "The repository asks for the Rust target {target} ({}). It is a rustup component, so it installs for your user only: rustup target add{toolchain_flag} {target}",
                self.source()
            )),
            level: if present { Level::Ok } else { Level::Fail },
            detail: if present {
                format!("installed for {toolchain}")
            } else {
                format!("missing for {toolchain}; {}", self.source())
            },
            fix: (!present).then(|| Fix {
                command: format!(
                    "PATH=\"${{CARGO_HOME:-$HOME/.cargo}}/bin:$PATH\"; rustup target add{toolchain_flag} {target}"
                ),
                root: false,
                why: format!("adds the Rust target {target} to {toolchain} (user-level, rustup)"),
            }),
        }
    }

    fn source(&self) -> String {
        format!(
            "asked for by repository `{}` in goway.toml or rust-toolchain.toml",
            crate::render::clean(&self.repo)
        )
    }

    fn package_checks(&self, facts: &BTreeMap<String, String>, out: &mut Vec<Check>) {
        let Some(manager) = facts.get("pkg.mgr").map(String::as_str) else {
            return;
        };
        let wanted = self.packages.of(manager);
        if wanted.is_empty() {
            let detail = if manager == "none" {
                "no apt, dnf or pacman on this host; install by hand what goway.toml [toolchain] packages lists"
                    .to_owned()
            } else {
                format!("goway.toml [toolchain] packages lists no {manager} packages for this host")
            };
            out.push(Check {
                name: "packages".to_owned(),
                explain: None,
                level: Level::Warn,
                detail,
                fix: None,
            });
            return;
        }
        let all = wanted.join(", ");
        for name in wanted {
            let present = facts
                .get(&format!("pkg.{name}"))
                .is_some_and(|v| v == "yes");
            let command = match manager {
                "apt" => format!("apt-get update && apt-get install -y {name}"),
                "dnf" => format!("dnf install -y {name}"),
                _ => format!("pacman -S --noconfirm {name}"),
            };
            out.push(Check {
                name: format!("pkg:{name}"),
                explain: Some(format!(
                    "The repository asks for the {manager} package {name} in goway.toml [toolchain] packages. Installing it needs root, so goway only does it with --fix --rsudo after you confirm the exact commands."
                )),
                level: if present { Level::Ok } else { Level::Fail },
                detail: if present {
                    "installed".to_owned()
                } else {
                    format!("missing; {}", self.source())
                },
                fix: (!present).then(|| Fix {
                    command,
                    root: true,
                    why: format!(
                        "from repository `{}` goway.toml [toolchain] packages ({manager}): {all}; system packages need root",
                        crate::render::clean(&self.repo)
                    ),
                }),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prereqs() -> Prereqs {
        Prereqs {
            rust_targets: vec!["x86_64-pc-windows-gnu".to_owned()],
            channel: Some("1.98.0".to_owned()),
            packages: Packages {
                apt: vec!["gcc-mingw-w64-x86-64".to_owned()],
                dnf: vec!["mingw64-gcc".to_owned()],
                pacman: vec!["mingw-w64-gcc".to_owned()],
            },
            repo: "frob-v2".to_owned(),
        }
    }

    fn facts(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    // frob:tests crates/goway/src/doctor/prereq.rs::Prereqs.validate
    #[test]
    fn names_are_validated_against_each_package_managers_syntax() {
        let path = std::path::Path::new("goway.toml");
        assert!(prereqs().validate(path).is_ok());
        let hostile = [
            "--allow-unauthenticated",
            "-y",
            "a b",
            "a;b",
            "$(id)",
            "../x",
            "/etc/passwd",
            "a`b`",
            "a&&b",
            "a'b",
            "",
        ];
        for name in hostile {
            for manager in ["apt", "dnf", "pacman"] {
                let mut p = Prereqs::default();
                match manager {
                    "apt" => p.packages.apt = vec![name.to_owned()],
                    "dnf" => p.packages.dnf = vec![name.to_owned()],
                    _ => p.packages.pacman = vec![name.to_owned()],
                }
                let err = p.validate(path).unwrap_err().to_string();
                assert!(err.contains(manager), "{name}: {err}");
            }
            let p = Prereqs {
                rust_targets: vec![name.to_owned()],
                ..Prereqs::default()
            };
            assert!(p.validate(path).is_err(), "target {name}");
        }
        let p = Prereqs {
            channel: Some("1.0; rm".to_owned()),
            ..Prereqs::default()
        };
        assert!(p.validate(path).is_err());
        // Debian names are lower case; RPM and Arch ones may not be.
        assert!(!valid_apt("GCC"));
        assert!(valid_dnf("GCC") && valid_pacman("libfoo++"));
    }

    // frob:tests crates/goway/src/doctor/prereq.rs::Prereqs.probe_script
    #[test]
    fn the_probe_names_the_pinned_toolchain_and_every_package_and_wraps_the_doctor_call() {
        let script = prereqs().probe_script().unwrap();
        assert!(script.contains("--toolchain '1.98.0'"), "{script}");
        assert!(
            script.contains("for p in gcc-mingw-w64-x86-64;"),
            "{script}"
        );
        assert!(script.contains("for p in mingw64-gcc;"), "{script}");
        assert!(Prereqs::default().probe_script().is_none());
        assert_eq!(Prereqs::default().wrap("base"), "base");
        let wrapped = prereqs().wrap("base");
        assert!(wrapped.starts_with("base; bash -c '"), "{wrapped}");
        // The wrapped command is valid shell.
        let ok = std::process::Command::new("bash")
            .args(["-n", "-c", &wrapped.replacen("base", "true", 1)])
            .status()
            .map(|s| s.success());
        assert_ne!(ok.ok(), Some(false));
    }

    // frob:tests crates/goway/src/doctor/prereq.rs::Prereqs.checks
    #[test]
    fn a_missing_target_is_a_user_fix_and_a_missing_package_is_a_root_fix_naming_the_repository() {
        let f = facts(&[
            ("rt.status", "ok"),
            ("rt.installed", "x86_64-unknown-linux-gnu "),
            ("pkg.mgr", "apt"),
            ("pkg.gcc-mingw-w64-x86-64", "no"),
        ]);
        let checks = prereqs().checks(&f);
        let target = checks
            .iter()
            .find(|c| c.name == "target:x86_64-pc-windows-gnu")
            .unwrap();
        assert_eq!(target.level, Level::Fail);
        let fix = target.fix.as_ref().unwrap();
        assert!(!fix.root);
        assert!(
            fix.command
                .ends_with("rustup target add --toolchain 1.98.0 x86_64-pc-windows-gnu"),
            "{}",
            fix.command
        );
        let pkg = checks
            .iter()
            .find(|c| c.name == "pkg:gcc-mingw-w64-x86-64")
            .unwrap();
        let fix = pkg.fix.as_ref().unwrap();
        assert!(fix.root);
        assert_eq!(
            fix.command,
            "apt-get update && apt-get install -y gcc-mingw-w64-x86-64"
        );
        assert!(fix.why.contains("repository `frob-v2`"), "{}", fix.why);
        assert!(fix.why.contains("goway.toml"), "{}", fix.why);
        assert!(fix.why.contains("gcc-mingw-w64-x86-64"), "{}", fix.why);
        // The confirmation prompt names the repository and goway.toml as the source.
        let prompt =
            super::super::output::prompt_text("helios", &[(pkg.name.clone(), fix.clone())], false);
        assert!(
            prompt.contains("repository `frob-v2` goway.toml"),
            "{prompt}"
        );
        assert!(prompt.contains("gcc-mingw-w64-x86-64"), "{prompt}");
    }

    #[test]
    fn present_things_pass_and_hosts_that_did_not_answer_get_no_rows() {
        let f = facts(&[
            ("rt.status", "ok"),
            ("rt.installed", "x86_64-pc-windows-gnu"),
            ("pkg.mgr", "dnf"),
            ("pkg.mingw64-gcc", "yes"),
        ]);
        let checks = prereqs().checks(&f);
        assert_eq!(checks.len(), 2);
        assert!(
            checks
                .iter()
                .all(|c| c.level == Level::Ok && c.fix.is_none())
        );
        assert!(prereqs().checks(&BTreeMap::new()).is_empty());
        let none = prereqs().checks(&facts(&[("rt.status", "no-rustup"), ("pkg.mgr", "none")]));
        assert!(
            none.iter()
                .all(|c| c.level == Level::Warn && c.fix.is_none())
        );
        assert_eq!(none.len(), 2);
    }
}
