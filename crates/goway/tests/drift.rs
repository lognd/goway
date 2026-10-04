//! Fleet drift: the versions table, what counts as disagreement, the laptop
//! column, and the cache doctor leaves for later runs.
#![cfg(unix)]

mod common;

use std::collections::{BTreeMap, BTreeSet};

use goway::drift::{Observed, drifting, table_lines};

fn host(name: &str, tools: &[(&str, &str)]) -> Observed {
    Observed {
        host: name.to_owned(),
        versions: tools
            .iter()
            .map(|(t, v)| ((*t).to_owned(), (*v).to_owned()))
            .collect(),
    }
}

fn names(tools: &[&str]) -> Vec<String> {
    tools.iter().map(|t| (*t).to_owned()).collect()
}

#[test]
fn a_major_difference_always_drifts_and_a_minor_one_only_for_compilers_and_pins() {
    let hosts = [
        host(
            "helios",
            &[
                ("cmake", "cmake version 3.28.3"),
                ("gcc", "gcc (Ubuntu 13.2.0) 13.2.0"),
                ("make", "GNU Make 4.3"),
            ],
        ),
        host(
            "orion",
            &[
                ("cmake", "cmake version 3.22.1"),
                ("gcc", "gcc (Ubuntu 12.3.0) 12.3.0"),
                ("make", "GNU Make 4.2"),
            ],
        ),
    ];
    let tools = names(&["cmake", "gcc", "make"]);
    let none = BTreeSet::new();
    let got = drifting(&tools, &none, &hosts);
    // cmake differs only in the minor (and is not pinned): fine. gcc differs
    // in the major. make differs in the minor and is not a compiler: fine.
    assert_eq!(got, [("gcc".to_owned(), "major: 13 vs 12".to_owned())]);
    let pinned: BTreeSet<String> = ["cmake".to_owned()].into();
    let got = drifting(&tools, &pinned, &hosts);
    assert_eq!(
        got,
        [
            ("cmake".to_owned(), "minor: 3.28 vs 3.22".to_owned()),
            ("gcc".to_owned(), "major: 13 vs 12".to_owned())
        ]
    );
}

#[test]
fn the_table_has_a_row_per_tool_a_laptop_column_and_marks_drift() {
    let hosts = [
        host(
            "helios",
            &[
                ("go", "go version go1.22.2 linux/amd64"),
                ("make", "GNU Make 4.3"),
            ],
        ),
        host("orion", &[("go", "go version go1.21.5 linux/amd64")]),
    ];
    let laptop: BTreeMap<String, String> = [(
        "go".to_owned(),
        "go version go1.22.2 linux/arm64".to_owned(),
    )]
    .into();
    let lines = table_lines(
        &names(&["go", "make"]),
        &BTreeSet::new(),
        &hosts,
        &laptop,
        false,
    );
    assert_eq!(
        lines.join("\n"),
        "tool  laptop   helios  orion    drift\n\
         go    1.22.2   1.22.2  1.21.5   DRIFT minor: 1.22 vs 1.21\n\
         make  missing  4.3     missing"
            .replace("\n         ", "\n")
    );
}

#[test]
fn a_missing_tool_on_one_host_is_not_drift() {
    let hosts = [
        host("helios", &[("go", "go version go1.22.2 linux/amd64")]),
        host("orion", &[]),
    ];
    assert!(drifting(&names(&["go"]), &BTreeSet::new(), &hosts).is_empty());
}

#[test]
fn doctor_shows_the_versions_table_with_all_and_caches_what_it_saw() {
    let w = common::world();
    std::fs::write(
        w.repo.join("goway.toml"),
        "[toolchain]\ntools = [\"git\"]\n",
    )
    .unwrap();
    let out = w.run(&["doctor", "--all"]);
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    let header = text
        .lines()
        .find(|l| l.split_whitespace().next() == Some("tool"))
        .unwrap_or_else(|| panic!("no versions table\n{text}"));
    assert!(
        header.contains("laptop") && header.contains("local") && header.contains("drift"),
        "{header}"
    );
    assert!(
        text.lines()
            .any(|l| l.split_whitespace().next() == Some("git")
                && l.split_whitespace()
                    .nth(1)
                    .is_some_and(|v| v.chars().next().is_some_and(|c| c.is_ascii_digit()))),
        "{text}"
    );
    let state = std::fs::read_to_string(w.root.join("state/hosts.json")).unwrap();
    let json: serde_json::Value = serde_json::from_str(&state).unwrap();
    let cached = &json["tool_versions"]["local"];
    assert!(cached["captured"].as_u64().unwrap() > 0, "{state}");
    assert!(
        cached["versions"]["git"].as_str().unwrap().contains("git"),
        "{state}"
    );
}

#[test]
fn all_hosts_needs_fix_and_refuses_a_named_host() {
    let w = common::world();
    assert_eq!(w.run(&["doctor", "--all-hosts"]).status.code(), Some(2));
    assert_eq!(
        w.run(&["doctor", "local", "--fix", "--all-hosts"])
            .status
            .code(),
        Some(2)
    );
}
