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

#[test]
fn pyproject_builds_the_bin_with_maturin_and_takes_the_version_from_cargo() {
    let py: toml::Table = read("pyproject.toml").parse().unwrap();
    assert_eq!(py["project"]["name"].as_str(), Some("goway"));
    assert_eq!(py["project"]["license"].as_str(), Some("MIT"));
    assert_eq!(py["project"]["dynamic"][0].as_str(), Some("version"));
    assert_eq!(
        py["build-system"]["build-backend"].as_str(),
        Some("maturin")
    );
    assert_eq!(py["tool"]["maturin"]["bindings"].as_str(), Some("bin"));
    assert_eq!(
        py["tool"]["maturin"]["manifest-path"].as_str(),
        Some("crates/goway/Cargo.toml")
    );
}

#[test]
fn release_publishes_wheels_to_pypi_with_trusted_publishing_after_the_github_release() {
    let release = read(".github/workflows/release.yml");
    let job = release.split("  pypi:").nth(1).expect("pypi job");
    assert!(job.contains("needs: [publish, wheels, sdist]"));
    assert!(job.contains("environment: pypi"));
    assert!(job.contains("id-token: write"));
    assert!(job.contains("pypa/gh-action-pypi-publish"));
    assert!(!job.contains("password"), "no stored token");
    for target in [
        "x86_64-unknown-linux-gnu",
        "aarch64-unknown-linux-gnu",
        "x86_64-unknown-linux-musl",
        "aarch64-unknown-linux-musl",
        "x86_64-pc-windows-msvc",
    ] {
        assert!(release.contains(&format!("target: {target}")), "{target}");
    }
    assert!(release.contains("command: sdist"));
}

#[test]
fn ci_builds_a_wheel_and_runs_goway_version_from_a_venv() {
    let ci = read(".github/workflows/ci.yml");
    assert!(ci.contains("PyO3/maturin-action"));
    assert!(ci.contains("goway --version"));
}

/// The text of one top-level job of the release workflow, up to the next job.
fn release_job(name: &str) -> String {
    let release = read(".github/workflows/release.yml");
    let jobs = release.split("\njobs:\n").nth(1).expect("jobs section");
    let header = format!("  {name}:");
    let mut out = Vec::new();
    let mut inside = false;
    for line in jobs.lines() {
        let is_header = line.starts_with("  ") && !line.starts_with("   ") && line.ends_with(':');
        if is_header {
            inside = line == header;
        }
        if inside {
            out.push(line);
        }
    }
    assert!(!out.is_empty(), "job {name} not found");
    out.join("\n")
}

#[test]
fn every_publishing_job_runs_only_on_a_version_tag_push() {
    let guard = "if: startsWith(github.ref, 'refs/tags/v') && github.event_name == 'push'";
    for job in ["publish", "crates-io", "pypi"] {
        assert!(release_job(job).contains(guard), "{job} lacks the tag guard");
    }
}

#[test]
fn only_the_publishing_jobs_publish_and_a_dry_run_is_possible() {
    let release = read(".github/workflows/release.yml");
    assert!(release.contains("workflow_dispatch:"));
    for job in ["linux", "windows", "wheels", "sdist", "checksums"] {
        let text = release_job(job);
        for needle in ["gh release create", "cargo publish", "gh-action-pypi-publish"] {
            assert!(!text.contains(needle), "{job} must not publish");
        }
    }
}
