//! Capacity-weighted sharding through the fake-ssh world: two "hosts" that
//! are this machine, one pretending to have 64 cores and the other 16.
#![cfg(unix)]

mod common;

use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt as _;

/// A fake ssh that makes `nproc` answer per target address.
const FAKE_SSH_PER_HOST: &str = r#"#!/bin/sh
while [ $# -gt 0 ]; do
  case "$1" in
    -o|-p|-l) shift 2 ;;
    --) shift; host=$1; shift; break ;;
    *) shift ;;
  esac
done
case "$host" in
  *127.0.0.2*) export FAKE_NPROC=16 ;;
  *) export FAKE_NPROC=64 ;;
esac
exec sh -c "$1"
"#;

fn shim(w: &common::World, name: &str, body: &str) {
    let path = w.bin.join(name);
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// Two hosts, `big` (64 cores) and `small` (16), and a stand-in test runner
/// named `fake-nextest` that prints the arguments it was given.
fn world() -> common::World {
    let w = common::world_with_ssh(FAKE_SSH_PER_HOST);
    let config = format!(
        "[defaults]\nremote_root = \"{}\"\ntarget_slots = 2\n\n[[host]]\nname = \"big\"\naddress = \"127.0.0.1\"\n\n[[host]]\nname = \"small\"\naddress = \"127.0.0.2\"\n",
        w.remote.display()
    );
    std::fs::write(w.config.join("config.toml"), config).unwrap();
    shim(&w, "nproc", "echo ${FAKE_NPROC:-4}");
    // A Mac's remote script asks sysctl for the core count; everything else passes through.
    shim(
        &w,
        "sysctl",
        "if [ \"$*\" = \"-n hw.ncpu\" ]; then echo ${FAKE_NPROC:-4}; else exec /usr/sbin/sysctl \"$@\"; fi",
    );
    shim(&w, "fake-nextest", "echo \"ARGS $*\"");
    w
}

fn partitions_by_host(stdout: &str) -> BTreeMap<String, Vec<String>> {
    let mut by: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for line in stdout.lines().filter(|l| l.contains("ARGS ")) {
        let host = line[1..line.find(']').unwrap()].trim().to_owned();
        let p = line.split("--partition ").nth(1).unwrap().trim().to_owned();
        by.entry(host).or_default().push(p);
    }
    by
}

// frob:tests crates/goway/src/shard.rs::run_sharded
// frob:tests crates/goway/src/runners.rs::plan_weighted
#[test]
fn a_bigger_host_runs_proportionally_more_partitions_and_together_they_cover_all() {
    let w = world();
    let report = w.root.join("report.json");
    let out = w.run(&[
        "run",
        "--shard",
        "2",
        "--report",
        report.to_str().unwrap(),
        "--",
        "fake-nextest",
        "run",
        "--workspace",
    ]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stderr}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let by = partitions_by_host(&stdout);
    assert_eq!(by.len(), 2, "{stdout}");
    // 64 vs 16 free cores: weights 4 and 1, so 5 partitions, 4 of them on the big host.
    let (big, small) = (&by["big"], &by["small"]);
    assert_eq!((big.len(), small.len()), (4, 1), "{by:?}");
    let mut all: Vec<&String> = big.iter().chain(small).collect();
    all.sort();
    assert_eq!(
        all,
        [
            "count:1/5",
            "count:2/5",
            "count:3/5",
            "count:4/5",
            "count:5/5"
        ],
        "every partition once"
    );
    assert!(
        stderr.contains("shares by free cores: big 4/5, small 1/5"),
        "{stderr}"
    );
    assert!(stderr.contains("80% share"), "{stderr}");
    // The report lists each shard's host, architecture and share.
    let r: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(report).unwrap()).unwrap();
    for shard in r["shards"].as_array().unwrap() {
        let host = shard["host"].as_str().unwrap();
        assert_eq!(shard["arch"], std::env::consts::ARCH);
        let share = &shard["share"];
        assert_eq!(share["total_weight"], 5);
        assert_eq!(share["weighted"], true);
        let (weight, fraction) = if host == "big" { (4, 0.8) } else { (1, 0.2) };
        assert_eq!(share["weight"], weight, "{host}");
        assert!((share["fraction"].as_f64().unwrap() - fraction).abs() < 1e-9);
        assert!(share["capacity"].as_f64().unwrap() > 0.0);
    }
}

// frob:tests crates/goway/src/shard.rs::run_sharded
#[test]
fn equal_capacity_and_native_splitters_keep_equal_shares() {
    // Same cores on both hosts: the plain `count:i/N` split, and a share of 1/2 each.
    let w = common::world();
    let mut config = std::fs::read_to_string(w.config.join("config.toml")).unwrap();
    config = config.replace("name = \"local\"", "name = \"alpha\"");
    config.push_str("\n[[host]]\nname = \"beta\"\naddress = \"127.0.0.1\"\n");
    std::fs::write(w.config.join("config.toml"), config).unwrap();
    shim(&w, "fake-nextest", "echo \"ARGS $*\"");
    let report = w.root.join("report.json");
    let out = w.run(&[
        "run",
        "--shard",
        "2",
        "--report",
        report.to_str().unwrap(),
        "--",
        "fake-nextest",
        "run",
    ]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let by = partitions_by_host(&String::from_utf8_lossy(&out.stdout));
    let mut parts: Vec<&str> = by.values().flatten().map(String::as_str).collect();
    parts.sort_unstable();
    assert_eq!(parts, ["count:1/2", "count:2/2"]);
    let r: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(report).unwrap()).unwrap();
    for shard in r["shards"].as_array().unwrap() {
        assert_eq!(shard["share"]["weight"], 1);
        assert_eq!(shard["share"]["total_weight"], 2);
        assert_eq!(shard["share"]["weighted"], false);
    }
}

// frob:tests crates/goway/src/shard.rs::run_sharded
#[test]
fn a_framework_that_cannot_split_by_weight_says_so_and_splits_equally() {
    let w = world();
    shim(&w, "vitest", "echo \"ARGS $*\"");
    let out = w.run(&["run", "--shard", "2", "--", "vitest", "run"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stderr}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("--shard=1/2") && stdout.contains("--shard=2/2"),
        "{stdout}"
    );
    assert!(stderr.contains("splits its shards equally"), "{stderr}");
}
