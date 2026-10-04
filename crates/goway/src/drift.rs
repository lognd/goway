//! Fleet drift: the versions of a project's tools on every host (and on this
//! laptop), side by side, and which of them disagree. Pure over version
//! text so the table and the verdict can be pinned by tests; the laptop
//! probe is the only part that runs anything.

use std::collections::{BTreeMap, BTreeSet};

use crate::doctor::first_version;
use crate::render;
use crate::state::State;

/// Tools whose minor version also matters: a different compiler minor can
/// change what a build produces. Other tools drift only across majors,
/// unless `goway.toml` pins them.
const COMPILERS: &[&str] = &[
    "gcc", "g++", "cc", "c++", "clang", "clang++", "rustc", "go", "java", "javac", "dotnet",
];

/// What one host reported: tool name to its first version line (absent or
/// empty when the tool is missing there).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Observed {
    /// The host's name.
    pub host: String,
    /// The tools' version lines.
    pub versions: BTreeMap<String, String>,
}

/// The `want.TOOL` facts of a probe, as tool name to version line.
pub fn versions_from_facts(
    facts: &BTreeMap<String, String>,
    tools: &[String],
) -> BTreeMap<String, String> {
    tools
        .iter()
        .filter_map(|t| {
            facts
                .get(&format!("want.{t}"))
                .filter(|v| !v.is_empty())
                .map(|v| (t.clone(), v.clone()))
        })
        .collect()
}

/// The short form shown in a cell: the first dotted version, else the text cut short.
fn short(line: &str) -> String {
    match first_version(line) {
        Some(v) => v.iter().map(u64::to_string).collect::<Vec<_>>().join("."),
        None => line.chars().take(20).collect(),
    }
}

/// How the hosts that have `tool` disagree, if they do: `major` always
/// counts, `minor` only for compilers and pinned tools.
fn disagreement(tool: &str, pinned: bool, found: &[(String, Vec<u64>)]) -> Option<String> {
    let majors: BTreeSet<u64> = found
        .iter()
        .filter_map(|(_, v)| v.first().copied())
        .collect();
    let minors: BTreeSet<(u64, u64)> = found
        .iter()
        .filter_map(|(_, v)| Some((*v.first()?, v.get(1).copied().unwrap_or(0))))
        .collect();
    let list = |f: &dyn Fn(&Vec<u64>) -> String| {
        let mut seen: Vec<String> = Vec::new();
        for (_, v) in found {
            let s = f(v);
            if !seen.contains(&s) {
                seen.push(s);
            }
        }
        seen.join(" vs ")
    };
    if majors.len() > 1 {
        Some(format!(
            "major: {}",
            list(&|v| v.first().map_or_else(String::new, u64::to_string))
        ))
    } else if minors.len() > 1 && (pinned || COMPILERS.contains(&tool)) {
        Some(format!(
            "minor: {}",
            list(&|v| {
                format!(
                    "{}.{}",
                    v.first().copied().unwrap_or(0),
                    v.get(1).copied().unwrap_or(0)
                )
            })
        ))
    } else {
        None
    }
}

/// The tools that drift across `hosts` and how, in tool order.
pub fn drifting(
    tools: &[String],
    pinned: &BTreeSet<String>,
    hosts: &[Observed],
) -> Vec<(String, String)> {
    tools
        .iter()
        .filter_map(|t| {
            let found: Vec<(String, Vec<u64>)> = hosts
                .iter()
                .filter_map(|h| {
                    h.versions
                        .get(t)
                        .and_then(|l| first_version(l))
                        .map(|v| (h.host.clone(), v))
                })
                .collect();
            disagreement(t, pinned.contains(t), &found).map(|d| (t.clone(), d))
        })
        .collect()
}

/// The table: a row per tool, a column for this laptop and one per host,
/// and the drift verdict last. Missing tools read `missing`.
pub fn table_lines(
    tools: &[String],
    pinned: &BTreeSet<String>,
    hosts: &[Observed],
    laptop: &BTreeMap<String, String>,
    plain: bool,
) -> Vec<String> {
    let drift = drifting(tools, pinned, hosts);
    let mut header = vec!["tool".to_owned(), "laptop".to_owned()];
    header.extend(hosts.iter().map(|h| h.host.clone()));
    header.push("drift".to_owned());
    let mut rows = vec![header];
    for t in tools {
        let cell = |line: Option<&String>| line.map_or_else(|| "missing".to_owned(), |l| short(l));
        let mut row = vec![t.clone(), cell(laptop.get(t))];
        row.extend(hosts.iter().map(|h| cell(h.versions.get(t))));
        row.push(
            drift
                .iter()
                .find(|(d, _)| d == t)
                .map_or_else(String::new, |(_, how)| format!("DRIFT {how}")),
        );
        rows.push(row);
    }
    if plain {
        render::plain_table(&rows)
    } else {
        render::format_table(&rows)
    }
}

/// The program and arguments that print `tool`'s version.
fn version_command(tool: &str) -> (&str, &'static [&'static str]) {
    match tool {
        "go" => ("go", &["version"]),
        "java" => ("java", &["-version"]),
        _ => (tool, &["--version"]),
    }
}

/// The first line of `tool`'s version report on this laptop, `None` when it
/// is missing, fails, or is too slow. The tool names come from the project's
/// own files and `goway.toml` and are validated names, never shell text (no
/// shell is involved). The probe runs in a neutral directory (never the
/// repository) with rustup auto-install off, so a project cannot pick the
/// program that answers.
pub fn laptop_version(tool: &str) -> Option<String> {
    let (program, args) = version_command(tool);
    let printed = crate::ecotools::bounded_output(program, args, &[])?;
    // `java -version` reports on stderr.
    let text = if printed.stdout.trim().is_empty() {
        printed.stderr
    } else {
        printed.stdout
    };
    text.lines()
        .find(|l| !l.trim().is_empty())
        .map(|l| l.trim().to_owned())
}

/// The laptop's versions of `tools` (those it has).
pub fn laptop_versions(tools: &[String]) -> BTreeMap<String, String> {
    tools
        .iter()
        .filter_map(|t| laptop_version(t).map(|v| (t.clone(), v)))
        .collect()
}

/// The tool versions of the host a run used, as `--report` records them.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Record {
    /// Where they come from: goway's cache of what `goway doctor` saw.
    pub source: &'static str,
    /// When doctor captured them, in seconds since the Unix epoch.
    pub captured: u64,
    /// Tool name to the first line of its version report.
    pub versions: BTreeMap<String, String>,
}

/// What a run says about the tool versions of the host it chose.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ForRun {
    /// One line when the chosen host differs from the rest of the fleet.
    pub note: Option<String>,
    /// The chosen host's versions, for the report.
    pub record: Option<Record>,
}

/// Whether any of `hosts` lacks a cache of `repo_id`'s tool versions younger than a day.
pub fn any_stale(state: &State, hosts: &[String], repo_id: &str, now: u64) -> bool {
    hosts.iter().any(|h| {
        state
            .tool_versions
            .get(&h.to_ascii_lowercase())
            .is_none_or(|c| {
                c.repo != repo_id || now.saturating_sub(c.captured) >= crate::state::TOOLS_MAX_AGE
            })
    })
}

/// Make the probes of this run ask for the project's tool versions where the
/// cache is stale. `tools` is only called (it reads project files) when
/// some host needs it; no tools or no repository id asks for nothing.
// frob:ticket 01M43AFK9N84ZE45CY5T8B9HQV
pub fn ask_where_stale(
    state: &mut State,
    hosts: &[String],
    repo_id: &str,
    now: u64,
    tools: impl FnOnce() -> Vec<String>,
) {
    if repo_id.is_empty() || !any_stale(state, hosts, repo_id, now) {
        return;
    }
    let tools = tools();
    if tools.is_empty() {
        return;
    }
    tracing::debug!(?tools, "stale tool versions: the probes will ask for them");
    state.want_tools = Some((repo_id.to_owned(), tools));
}

/// Record what the chosen `host` reported for the tools its probe was asked
/// about; true when the cache changed. A probe that was not asked, or a host
/// that reported nothing (an older helper), leaves the cache alone.
// frob:ticket 01M43AFK9N84ZE45CY5T8B9HQV
pub fn record_probed(
    state: &mut State,
    host: &str,
    now: u64,
    reported: &BTreeMap<String, String>,
) -> bool {
    let Some((repo_id, _)) = state.want_tools.clone() else {
        return false;
    };
    if reported.is_empty() || state.tools_to_probe(host, now).is_empty() {
        return false;
    }
    tracing::info!(
        host,
        tools = reported.len(),
        "tool versions refreshed by a run"
    );
    state.record_tools(host, &repo_id, now, reported.clone());
    true
}

/// Seconds as a short age: `3d`, `5h`, `12m`.
fn age(secs: u64) -> String {
    match secs {
        s if s >= 86_400 => format!("{}d", s / 86_400),
        s if s >= 3_600 => format!("{}h", s / 3_600),
        s => format!("{}m", s / 60),
    }
}

/// Compare the cached versions (written by `goway doctor` in this
/// repository) of the chosen `host` with the other hosts', at time `now`.
/// Nothing is probed: a run never waits for versions. Without a cache for
/// this repository on the chosen host the result is empty.
pub fn for_run(state: &State, host: &str, repo_id: &str, now: u64) -> ForRun {
    let key = host.to_ascii_lowercase();
    let Some(mine) = state
        .tool_versions
        .get(&key)
        .filter(|c| !repo_id.is_empty() && c.repo == repo_id)
    else {
        return ForRun::default();
    };
    let record = Some(Record {
        source: "goway doctor cache",
        captured: mine.captured,
        versions: mine.versions.clone(),
    });
    let fleet: Vec<Observed> = state
        .tool_versions
        .iter()
        .filter(|(_, c)| c.repo == repo_id)
        .map(|(h, c)| Observed {
            host: h.clone(),
            versions: c.versions.clone(),
        })
        .collect();
    let tools: Vec<String> = mine.versions.keys().cloned().collect();
    let drift = drifting(&tools, &BTreeSet::new(), &fleet);
    if drift.is_empty() {
        return ForRun { note: None, record };
    }
    let list: Vec<String> = drift
        .iter()
        .map(|(tool, how)| {
            let here = mine
                .versions
                .get(tool)
                .map_or_else(String::new, |l| short(l));
            format!("{tool} {here} here ({how})")
        })
        .collect();
    ForRun {
        note: Some(format!(
            "tool versions differ across your hosts: {} (cached by goway doctor {} ago; builds may behave differently here)",
            list.join(", "),
            age(now.saturating_sub(mine.captured))
        )),
        record,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    const NOW: u64 = 1_000_000;

    fn names(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_owned()).collect()
    }

    fn cached(state: &mut State, host: &str, repo: &str, at: u64) {
        state.record_tools(
            host,
            repo,
            at,
            BTreeMap::from([("cc".to_owned(), "cc 12".to_owned())]),
        );
    }

    // frob:tests crates/goway/src/drift.rs::ask_where_stale
    // frob:tests crates/goway/src/state.rs::State.tools_to_probe
    #[test]
    fn a_missing_or_day_old_cache_makes_the_probe_ask_and_a_fresh_one_does_not() {
        let hosts = names(&["helios"]);
        let mut empty = State::default();
        ask_where_stale(&mut empty, &hosts, "r1", NOW, || names(&["cc", "make"]));
        assert_eq!(empty.tools_to_probe("Helios", NOW), ["cc", "make"]);

        let mut aged = State::default();
        cached(&mut aged, "helios", "r1", NOW - 90_000);
        ask_where_stale(&mut aged, &hosts, "r1", NOW, || names(&["cc"]));
        assert_eq!(aged.tools_to_probe("helios", NOW), ["cc"]);

        let mut other_repo = State::default();
        cached(&mut other_repo, "helios", "r2", NOW - 10);
        ask_where_stale(&mut other_repo, &hosts, "r1", NOW, || names(&["cc"]));
        assert_eq!(other_repo.tools_to_probe("helios", NOW), ["cc"]);

        let mut fresh = State::default();
        cached(&mut fresh, "helios", "r1", NOW - 3_600);
        ask_where_stale(&mut fresh, &hosts, "r1", NOW, || {
            panic!("must not read the project")
        });
        assert!(fresh.tools_to_probe("helios", NOW).is_empty());
        let mut no_repo = State::default();
        ask_where_stale(&mut no_repo, &hosts, "", NOW, || panic!("no repository"));
        assert!(no_repo.want_tools.is_none());
    }

    // frob:tests crates/goway/src/pool.rs::probe_call
    #[test]
    fn the_probe_word_carries_only_safe_tool_names() {
        let config = Config::default();
        let args =
            |t: &[&str]| crate::pool::probe_call(&config, None, false, false, &names(t)).args;
        assert!(args(&[]).iter().all(|a| !a.starts_with("tools:")));
        let a = args(&["cc", "g++", "bad name", "x;rm", "cargo-nextest"]);
        assert!(
            a.contains(&"tools:cc,g++,cargo-nextest".to_owned()),
            "{a:?}"
        );
    }

    // frob:tests crates/goway/src/drift.rs::record_probed
    #[test]
    fn a_run_records_what_its_probe_reported_only_when_it_asked() {
        let hosts = names(&["helios"]);
        let reported = BTreeMap::from([("cc".to_owned(), "cc 13.2".to_owned())]);
        let mut state = State::default();
        assert!(
            !record_probed(&mut state, "helios", NOW, &reported),
            "not asked"
        );
        ask_where_stale(&mut state, &hosts, "r1", NOW, || names(&["cc"]));
        assert!(
            !record_probed(&mut state, "helios", NOW, &BTreeMap::new()),
            "silent host"
        );
        assert!(state.tool_versions.is_empty());
        assert!(record_probed(&mut state, "helios", NOW, &reported));
        let got = for_run(&state, "helios", "r1", NOW + 5);
        let rec = got.record.unwrap();
        assert_eq!((rec.captured, rec.versions), (NOW, reported));
        assert!(
            !record_probed(
                &mut state,
                "helios",
                NOW,
                &BTreeMap::from([("cc".to_owned(), "x".to_owned())])
            ),
            "fresh now: not asked again"
        );
    }

    // frob:tests crates/goway/src/facts.rs::parse_live
    #[test]
    fn the_probe_parser_keeps_reported_tool_versions_and_drops_missing_ones() {
        let p = crate::pool::parse_probe(
            "arch=x86_64\nhostname=h\ncores=4\nload1=0\nload5=0\nload15=0\njobs=0\nwant.cc=gcc 13.2\nwant.make=\n",
        )
        .unwrap();
        assert_eq!(
            p.facts.tools,
            BTreeMap::from([("cc".to_owned(), "gcc 13.2".to_owned())])
        );
    }
}
