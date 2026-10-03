+++
id = "01M41PH8825T7M1779CNW25FJX"
title = "Publish goway and goway-journal to crates.io from the release workflow"
type = "task"
category = "in-progress"
priority = "medium"
points = 2
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-03T20:15:51Z"
updated = "2026-10-03T20:31:17Z"
scope = ["Cargo.toml", "crates/*/Cargo.toml", ".github/workflows/release.yml", ".github/workflows/ci.yml", "docs/release.md", "README.md", "pyproject.toml", "crates/goway/tests/publishing.rs"]

[[acceptance]]
text = "Given a version tag, when the release workflow runs, then goway-journal and goway are published to crates.io after the GitHub release succeeds, and cargo publish --dry-run runs in CI on every push"
bound = true

[[acceptance]]
text = "Given goway-setup, when packaging, then it is never published (publish = false)"
bound = true
+++
