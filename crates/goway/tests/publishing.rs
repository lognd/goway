//! Publishing contract: which crates are published, and that CI and the
//! release workflow wire crates.io the way docs/release.md describes.

use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read(rel: &str) -> String {
    std::fs::read_to_string(root().join(rel)).unwrap()
}

fn manifest(crate_name: &str) -> toml::Table {
    read(&format!("crates/{crate_name}/Cargo.toml"))
        .parse()
        .unwrap()
}

fn publish_flag(crate_name: &str) -> Option<bool> {
    manifest(crate_name)["package"]
        .get("publish")
        .and_then(toml::Value::as_bool)
}

#[test]
fn only_goway_and_goway_journal_are_published() {
    let root: toml::Table = read("Cargo.toml").parse().unwrap();
    assert_eq!(
        root["workspace"]["package"]["publish"].as_bool(),
        Some(false)
    );
    assert_eq!(publish_flag("goway"), Some(true));
    assert_eq!(publish_flag("goway-journal"), Some(true));
    assert_eq!(publish_flag("goway-setup"), Some(false));
}

#[test]
fn journal_dependency_carries_a_version_for_crates_io() {
    let root: toml::Table = read("Cargo.toml").parse().unwrap();
    let dep = &root["workspace"]["dependencies"]["goway-journal"];
    assert_eq!(
        dep["version"].as_str(),
        root["workspace"]["package"]["version"].as_str()
    );
    assert!(dep.get("path").is_some());
}

#[test]
fn ci_dry_runs_the_publish_on_every_push() {
    let ci = read(".github/workflows/ci.yml");
    assert!(ci.contains("cargo publish --workspace --dry-run --exclude goway-setup"));
}

#[test]
fn release_publishes_to_crates_io_after_the_github_release() {
    let release = read(".github/workflows/release.yml");
    let job = release.split("  crates-io:").nth(1).expect("crates-io job");
    assert!(job.contains("needs: [publish]"));
    assert!(job.contains("environment: crates-io"));
    assert!(job.contains("CARGO_REGISTRY_TOKEN: ${{ secrets.CARGO_REGISTRY_TOKEN }}"));
    assert!(job.contains("cargo publish --workspace --exclude goway-setup"));
}
