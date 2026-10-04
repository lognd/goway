//! `goway run --needs` and `--prefers` through the fake-ssh world.
#![cfg(unix)]

mod common;

fn report_of(w: &common::World, name: &str) -> (std::path::PathBuf, String) {
    let path = w.root.join(name);
    (path.clone(), path.to_str().unwrap().to_owned())
}

// frob:tests crates/goway/src/pool.rs::choose
#[test]
fn a_host_that_lacks_a_need_is_never_used_and_the_error_lists_why() {
    let w = common::world();
    let out = w.run(&[
        "run",
        "--needs",
        "cores>=999999,docker=1,bogus",
        "--",
        "true",
    ]);
    assert_eq!(out.status.code(), Some(125));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("valid terms"), "{err}");
    let out = w.run(&["run", "--needs", "cores>=999999", "--", "true"]);
    assert_eq!(out.status.code(), Some(125));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("no host meets the requirements"), "{err}");
    assert!(err.contains("local: lacks cores>=999999: has "), "{err}");
    assert!(
        w.work_dirs().is_empty(),
        "nothing is synced for an unmet need"
    );
}

// frob:tests crates/goway/src/run.rs::Report
#[test]
fn the_report_records_the_facts_that_met_needs_and_preferences() {
    let w = common::world();
    let os = common::host_os();
    let os_term = format!("os={os}");
    let (path, arg) = report_of(&w, "report.json");
    let out = w.run(&[
        "run",
        "--needs",
        &format!("cores>=1,os={os}"),
        "--prefers",
        "gpu=rocm,mem>=1M",
        "--report",
        &arg,
        "--",
        "true",
    ]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let report: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let matched = report["matched"].as_array().unwrap();
    let terms: Vec<(&str, &str)> = matched
        .iter()
        .map(|m| (m["kind"].as_str().unwrap(), m["term"].as_str().unwrap()))
        .collect();
    assert_eq!(
        terms,
        [
            ("need", "cores>=1"),
            ("need", os_term.as_str()),
            ("prefer", "mem>=1M")
        ]
    );
    assert_eq!(matched[1]["fact"].as_str().unwrap(), os);
    assert!(matched[0]["fact"].as_str().unwrap().ends_with(" cores"));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("running on local"), "{err}");
    assert!(
        err.contains("cores>=1 ("),
        "the running-on line names the matched facts: {err}"
    );
}

// frob:tests crates/goway/src/shard.rs::run_sharded
#[test]
fn sharded_runs_need_every_host_to_qualify_and_report_each_hosts_facts() {
    let w = common::world();
    let mut config = std::fs::read_to_string(w.config.join("config.toml")).unwrap();
    config = config.replace("name = \"local\"", "name = \"alpha\"");
    config.push_str("\n[[host]]\nname = \"beta\"\naddress = \"127.0.0.1\"\n");
    std::fs::write(w.config.join("config.toml"), config).unwrap();
    let (path, arg) = report_of(&w, "shards.json");
    let out = w.run(&[
        "run", "--shard", "2", "--needs", "cores>=1", "--report", &arg, "--", "true",
    ]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let report: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    for shard in report["shards"].as_array().unwrap() {
        assert_eq!(shard["matched"][0]["term"], "cores>=1");
    }
    let out = w.run(&[
        "run",
        "--shard",
        "2",
        "--needs",
        "cores>=999999",
        "--",
        "true",
    ]);
    assert_eq!(out.status.code(), Some(125));
    assert!(String::from_utf8_lossy(&out.stderr).contains("each meet --needs"));
    assert!(w.work_dirs().is_empty());
}
