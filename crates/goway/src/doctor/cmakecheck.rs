//! doctor's `CMake` checks: what `CMake` itself says the project needs on a
//! helper (see [`crate::cmakeapi`]), as checks with the exact install command.
//!
//! The File API replies of the project's slot trees are read on every
//! `goway doctor` (nothing runs there). `goway doctor --configure` also syncs
//! the work tree as a snapshot and runs one traced configure of it on the
//! helper, in the snapshot's own labelled work directory, which the helper
//! removes afterwards (gc covers it if that fails). Nothing configures on
//! this machine, in the user's work tree, or on a Windows or this-machine host.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use super::{Check, Fix, Level, install};
use crate::cmakeapi::{self, Analysis, Bundle, Presence, Replies, SystemNeed, Trace};
use crate::config::Config;
use crate::repo::Repo;
use crate::resolve::Found;
use crate::run::Env;
use crate::sync::{SshTransport, Transport};
use crate::transport::Kind;

/// Seconds a doctor configure may take on a helper.
const CONFIGURE_SECONDS: u32 = 300;

/// Whether the project at `repo` is a `CMake` project (a `CMakeLists.txt` at its root).
pub(super) fn is_cmake_project(repo: &Repo) -> bool {
    repo.root.join("CMakeLists.txt").is_file()
}

/// What doctor asks of one helper.
pub(super) struct Ask<'a> {
    pub env: &'a Env<'a>,
    pub config: &'a Config,
    pub repo: &'a Repo,
    /// Also run a traced configure there (`--configure`).
    pub configure: bool,
}

fn check(
    name: &str,
    level: Level,
    detail: String,
    fix: Option<Fix>,
    explain: Option<String>,
) -> Check {
    Check {
        name: name.to_owned(),
        level,
        detail,
        fix,
        explain,
    }
}

/// The `CMake` checks for one reachable helper: none when it is not a Unix helper on another machine.
pub(super) fn host_checks(
    ask: &Ask<'_>,
    found: &Found,
    facts: &BTreeMap<String, String>,
) -> Vec<Check> {
    if found.is_local() || found.kind != Kind::Unix {
        tracing::debug!(host = %found.target.name, "no CMake checks here: not a helper that runs the remote script");
        return Vec::new();
    }
    let transport = SshTransport::of(found, ask.env.settings);
    let mut out = Vec::new();
    let mut replies = read_slot_replies(ask, &transport, &mut out);
    let (mut trace, mut stderr, mut rc) = (None, None, None);
    if ask.configure {
        match configure(ask, found, &transport) {
            Ok(bundle) => {
                rc = bundle.rc;
                stderr = bundle
                    .files
                    .get("stderr")
                    .map(|b| String::from_utf8_lossy(b).into_owned());
                trace = parse_trace_of(&bundle, &mut out);
                if let Some(build) = bundle.builds.iter().find(|b| b.label == "configure") {
                    replies = parse_replies(build, &mut out).or(replies);
                }
            }
            Err(e) => out.push(check("CMake configure", Level::Warn, e, None, None)),
        }
    }
    if replies.is_none() && trace.is_none() && rc.is_none() {
        tracing::debug!("nothing from CMake to check");
        return out;
    }
    let analysis = cmakeapi::analyse(replies.as_ref(), trace.as_ref(), stderr.as_deref(), rc);
    out.extend(checks_from(
        &analysis,
        facts,
        stderr.as_deref(),
        ask.configure,
    ));
    out
}

/// The newest File API replies the helper's slot trees hold, or a warning why they cannot be read.
fn read_slot_replies(
    ask: &Ask<'_>,
    transport: &SshTransport<'_>,
    out: &mut Vec<Check>,
) -> Option<Replies> {
    let call = cmakeapi::replies_call(&ask.config.defaults.remote_root, &ask.repo.id);
    let bundle = match transport
        .output(&call)
        .map_err(|e| e.to_string())
        .and_then(|b| cmakeapi::parse_bundle(&b).map_err(|e| e.to_string()))
    {
        Ok(b) => b,
        Err(e) => {
            out.push(check("CMake replies", Level::Warn, e, None, None));
            return None;
        }
    };
    let Some(build) = bundle.builds.first() else {
        tracing::debug!("no File API replies in the slot trees yet");
        return None;
    };
    parse_replies(build, out)
}

/// Parse one build directory's replies; a warning when they are not what `CMake` documents.
fn parse_replies(build: &cmakeapi::BuildReplies, out: &mut Vec<Check>) -> Option<Replies> {
    match cmakeapi::read_replies(&build.files) {
        Ok(r) => Some(r),
        Err(e) => {
            out.push(check(
                "CMake replies",
                Level::Warn,
                e.to_string(),
                None,
                None,
            ));
            None
        }
    }
}

/// Parse a configure's trace; a warning when it is not valid.
fn parse_trace_of(bundle: &Bundle, out: &mut Vec<Check>) -> Option<Trace> {
    let text = bundle.files.get("trace.jsonl")?;
    match cmakeapi::parse_trace(&String::from_utf8_lossy(text)) {
        Ok(t) => Some(t),
        Err(e) => {
            out.push(check("CMake trace", Level::Warn, e.to_string(), None, None));
            None
        }
    }
}

/// Sync the work tree as a snapshot and run the traced configure on the helper.
fn configure(ask: &Ask<'_>, found: &Found, transport: &SshTransport<'_>) -> Result<Bundle, String> {
    let run_id = crate::run::new_run_id();
    tracing::info!(host = %found.target.name, %run_id, "doctor --configure: syncing a snapshot");
    crate::run::sync_snapshot(ask.env, ask.config, ask.repo, found, &run_id, false, None)
        .map_err(|e| e.to_string())?;
    let bytes = transport
        .output(&cmakeapi::configure_call(
            &ask.config.defaults.remote_root,
            &run_id,
            CONFIGURE_SECONDS,
        ))
        .map_err(|e| e.to_string())?;
    cmakeapi::parse_bundle(&bytes).map_err(|e| e.to_string())
}

/// The one line saying what `CMake` found: its version, compilers and fetched dependencies.
fn summary(a: &Analysis) -> String {
    let compilers: Vec<String> = a
        .compilers
        .iter()
        .map(|c| {
            format!(
                "{} {} {}",
                c.language,
                c.id.as_deref().unwrap_or("?"),
                c.version.as_deref().unwrap_or("")
            )
            .trim()
            .to_owned()
        })
        .collect();
    let mut detail = format!("cmake {}", a.cmake_version.as_deref().unwrap_or("?"));
    if !compilers.is_empty() {
        let _ = write!(detail, "; compilers {}", compilers.join(", "));
    }
    if !a.fetched.is_empty() {
        let names: Vec<&str> = a.fetched.iter().map(|f| f.name.as_str()).collect();
        let _ = write!(
            detail,
            "; downloaded by CMake itself, no system package: {}",
            names.join(", ")
        );
    }
    detail
}

/// The check for a package `CMake` looked for and did not find, with this host's install command.
fn missing_check(
    need: &SystemNeed,
    facts: &BTreeMap<String, String>,
    stderr: Option<&str>,
) -> Check {
    let level = if need.required {
        Level::Fail
    } else {
        Level::Warn
    };
    let (detail, fix) = match need.package {
        Some(p) => (
            format!("missing ({}); install {}", need.why, p.apt),
            Some(Fix {
                command: install(facts, p.apt, p.dnf, p.pacman),
                root: true,
                why: format!(
                    "installs the development package CMake's {} needs",
                    need.name
                ),
            }),
        ),
        None => (
            format!(
                "missing ({}); install its development files with the system package manager",
                need.why
            ),
            None,
        ),
    };
    check(
        &format!("cmake: {}", need.name),
        level,
        detail,
        fix,
        stderr.map(tail),
    )
}

/// The checks an [`Analysis`] gives, with this host's install commands.
pub(super) fn checks_from(
    a: &Analysis,
    facts: &BTreeMap<String, String>,
    stderr: Option<&str>,
    configured: bool,
) -> Vec<Check> {
    let mut out = Vec::new();
    if !a.configure_failed {
        let origin = if configured {
            "CMake configure"
        } else {
            "CMake replies"
        };
        out.push(check(origin, Level::Ok, summary(a), None, None));
    }
    for need in &a.system {
        match need.presence {
            Presence::Present => out.push(check(
                &format!("cmake: {}", need.name),
                Level::Ok,
                format!("found ({})", need.why),
                None,
                None,
            )),
            Presence::Unknown => {}
            Presence::Missing => out.push(missing_check(need, facts, stderr)),
        }
    }
    for program in &a.missing_programs {
        let package = (program == "pkg-config")
            .then(|| cmakeapi::LibPackage::by_cmake("PkgConfig"))
            .flatten();
        let fix = package.map(|p| Fix {
            command: install(facts, p.apt, p.dnf, p.pacman),
            root: true,
            why: format!("CMake looked for {program} and did not find it"),
        });
        out.push(check(
            &format!("cmake: {program}"),
            Level::Warn,
            "missing; CMake looked for it".to_owned(),
            fix,
            None,
        ));
    }
    if a.configure_failed && !out.iter().any(|c| c.level == Level::Fail) {
        out.push(check(
            "CMake configure",
            Level::Fail,
            "failed for a reason goway cannot name; see --explain".to_owned(),
            None,
            stderr.map(tail),
        ));
    }
    out
}

/// The last lines of a configure's stderr, for `--explain`.
fn tail(stderr: &str) -> String {
    let lines: Vec<&str> = stderr.lines().collect();
    lines[lines.len().saturating_sub(25)..].join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cmakeapi::{LibPackage, SystemNeed};

    fn facts(apt: bool) -> BTreeMap<String, String> {
        let mut f = BTreeMap::new();
        if apt {
            f.insert("tool.apt-get".to_owned(), "/usr/bin/apt-get".to_owned());
        }
        f
    }

    fn analysis(system: Vec<SystemNeed>) -> Analysis {
        Analysis {
            cmake_version: Some("3.28.3".to_owned()),
            system,
            ..Analysis::default()
        }
    }

    // frob:tests crates/goway/src/doctor/cmakecheck.rs::checks_from
    #[test]
    fn a_missing_gtest_is_a_failure_with_the_exact_apt_command_and_a_found_one_is_just_ok() {
        let gtest = SystemNeed {
            name: "GTest".to_owned(),
            presence: Presence::Missing,
            package: LibPackage::by_cmake("GTest"),
            why: "find_package(GTest) at CMakeLists.txt:3".to_owned(),
            required: true,
        };
        let checks = checks_from(
            &analysis(vec![gtest.clone()]),
            &facts(true),
            Some("Could NOT find GTest\n"),
            true,
        );
        let c = checks.iter().find(|c| c.name == "cmake: GTest").unwrap();
        assert_eq!(c.level, Level::Fail);
        assert!(c.detail.contains("libgtest-dev"), "{}", c.detail);
        let fix = c.fix.as_ref().unwrap();
        assert!(fix.root);
        assert_eq!(
            fix.command,
            "apt-get update && apt-get install -y libgtest-dev"
        );
        assert!(
            c.explain
                .as_deref()
                .unwrap()
                .contains("Could NOT find GTest")
        );
        // Optional and missing: only a warning; another manager, another command.
        let optional = SystemNeed {
            required: false,
            ..gtest.clone()
        };
        let w = checks_from(
            &analysis(vec![optional]),
            &BTreeMap::from([("tool.dnf".to_owned(), "/usr/bin/dnf".to_owned())]),
            None,
            false,
        );
        let c = w.iter().find(|c| c.name == "cmake: GTest").unwrap();
        assert_eq!(c.level, Level::Warn);
        assert_eq!(
            c.fix.as_ref().unwrap().command,
            "dnf install -y gtest-devel"
        );
        // Found: ok, no fix.
        let present = SystemNeed {
            presence: Presence::Present,
            ..gtest
        };
        let ok = checks_from(&analysis(vec![present]), &facts(true), None, false);
        assert!(
            ok.iter().all(|c| c.level == Level::Ok && c.fix.is_none()),
            "{ok:?}"
        );
    }

    // frob:tests crates/goway/src/doctor/cmakecheck.rs::checks_from
    #[test]
    fn fetched_dependencies_need_no_package_and_a_failed_configure_without_a_name_still_fails() {
        let a = Analysis {
            fetched: vec![cmakeapi::Fetched {
                name: "googletest".to_owned(),
                via: "FetchContent",
            }],
            ..analysis(Vec::new())
        };
        let checks = checks_from(&a, &facts(true), None, false);
        assert_eq!(checks.len(), 1);
        assert!(
            checks[0].detail.contains("no system package: googletest"),
            "{}",
            checks[0].detail
        );
        let failed = Analysis {
            configure_failed: true,
            ..analysis(Vec::new())
        };
        let checks = checks_from(&failed, &facts(true), Some("boom\n"), true);
        assert_eq!(checks.len(), 1);
        assert_eq!(
            (checks[0].level, checks[0].name.as_str()),
            (Level::Fail, "CMake configure")
        );
    }
}
