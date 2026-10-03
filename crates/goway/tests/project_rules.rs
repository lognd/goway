//! `goway.toml` rules and host labels through the fake-ssh world.
#![cfg(unix)]

mod common;

fn write_rules(w: &common::World, text: &str) {
    std::fs::write(w.repo.join("goway.toml"), text).unwrap();
}

fn stderr(out: &std::process::Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

// frob:tests crates/goway/src/project.rs::selection_for
#[test]
fn a_rule_applies_is_named_and_the_command_line_wins() {
    let w = common::world();
    write_rules(
        &w,
        "[[rule]]\ncommand = \"true*\"\nneeds = [\"cores>=999999\"]\nprefers = [\"os=linux\"]\n",
    );
    // The rule alone excludes the only host.
    let out = w.run(&["run", "--", "true"]);
    assert_eq!(out.status.code(), Some(125));
    let err = stderr(&out);
    assert!(
        err.contains(
            "goway.toml rule 1 (command = \"true*\") applies: needs cores>=999999 prefers os=linux"
        ),
        "{err}"
    );
    assert!(err.contains("lacks cores>=999999"), "{err}");
    // The command line's `cores>=1` replaces the rule's term of the same key.
    let report = w.root.join("report.json");
    let out = w.run(&[
        "run",
        "--needs",
        "cores>=1",
        "--report",
        report.to_str().unwrap(),
        "--",
        "true",
    ]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("goway.toml rule 1"),
        "{}",
        stderr(&out)
    );
    let r: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&report).unwrap()).unwrap();
    assert_eq!(r["rule"]["rule"], 1);
    assert_eq!(r["rule"]["command"], "true*");
    let terms: Vec<&str> = r["matched"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["term"].as_str().unwrap())
        .collect();
    assert_eq!(terms, ["cores>=1", "os=linux"]);
    // A command no rule matches runs without needs and without a note.
    let out = w.run(&["run", "--", "echo", "hi"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(!stderr(&out).contains("goway.toml"), "{}", stderr(&out));
}

// frob:tests crates/goway/src/project.rs::Rules
#[test]
fn a_goway_toml_with_hosts_or_unknown_keys_is_refused_with_file_and_line() {
    let w = common::world();
    for (text, line) in [
        (
            "[[rule]]\ncommand = \"true\"\nhost = \"helios\"\n",
            "line 3",
        ),
        (
            "[[host]]\nname = \"helios\"\naddress = \"192.0.2.9\"\n",
            "line 1",
        ),
    ] {
        write_rules(&w, text);
        let out = w.run(&["run", "--", "true"]);
        assert_eq!(out.status.code(), Some(125));
        let err = stderr(&out);
        assert!(err.contains("goway.toml"), "{err}");
        assert!(err.contains(line), "{err}");
        assert!(
            w.work_dirs().is_empty(),
            "nothing runs under a bad goway.toml"
        );
    }
}

// frob:tests crates/goway/src/needs.rs::Term
#[test]
fn only_hosts_with_the_label_qualify() {
    let w = common::world();
    let mut config = std::fs::read_to_string(w.config.join("config.toml")).unwrap();
    config = config.replace(
        "name = \"local\"",
        "name = \"alpha\"\nlabels = [\"gpu-box\"]",
    );
    config.push_str("\n[[host]]\nname = \"beta\"\naddress = \"127.0.0.1\"\n");
    std::fs::write(w.config.join("config.toml"), config).unwrap();
    for _ in 0..3 {
        let report = w.root.join("report.json");
        let out = w.run(&[
            "run",
            "--needs",
            "label=gpu-box",
            "--report",
            report.to_str().unwrap(),
            "--",
            "true",
        ]);
        assert!(out.status.success(), "{}", stderr(&out));
        let r: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&report).unwrap()).unwrap();
        assert_eq!(r["host"], "alpha", "beta has no such label");
    }
    // The same through a rule, and through sharding (only one host qualifies).
    write_rules(
        &w,
        "[[rule]]\ncommand = \"true\"\nneeds = [\"label=gpu-box\"]\n",
    );
    let out = w.run(&["run", "--", "true"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let out = w.run(&["run", "--shard", "2", "--", "true"]);
    assert_eq!(out.status.code(), Some(125));
    let err = stderr(&out);
    assert!(
        err.contains("beta: lacks label=gpu-box: not labelled so"),
        "{err}"
    );
    // An unknown label excludes everyone; a malformed one is a config error.
    let out = w.run(&["run", "--needs", "label=missing", "--", "echo", "x"]);
    assert_eq!(out.status.code(), Some(125));
    assert!(stderr(&out).contains("lacks label=missing"));
    let config = std::fs::read_to_string(w.config.join("config.toml")).unwrap();
    std::fs::write(
        w.config.join("config.toml"),
        config.replace("gpu-box\"]", "bad label\"]"),
    )
    .unwrap();
    let out = w.run(&["run", "--", "echo", "x"]);
    assert_eq!(out.status.code(), Some(125));
    assert!(
        stderr(&out).contains("label `bad label` of host `alpha`"),
        "{}",
        stderr(&out)
    );
}
