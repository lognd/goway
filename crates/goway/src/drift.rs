//! Fleet drift: the versions of a project's tools on every host (and on this
//! laptop), side by side, and which of them disagree. Pure over version
//! text so the table and the verdict can be pinned by tests; the laptop
//! probe is the only part that runs anything.

use std::collections::{BTreeMap, BTreeSet};

use crate::doctor::first_version;
use crate::render;

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
/// shell is involved).
pub fn laptop_version(tool: &str) -> Option<String> {
    let (program, args) = version_command(tool);
    let printed = crate::ecotools::bounded_output(program, args, None, &[])?;
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
