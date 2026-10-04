+++
id = "01M44C7W7BZNCC912JGPA6WM2T"
title = "Audit3 H1: goway never runs a repository-selected program on the laptop"
type = "security"
category = "done"
outcome = "done"
priority = "high"
points = 5
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-04T21:13:41Z"
updated = "2026-10-04T21:29:21Z"
labels = ["security"]
scope = ["crates/goway/src/ecotools.rs", "crates/goway/src/drift.rs", "crates/goway/src/repo.rs", "crates/goway/src/sync.rs", "crates/goway/src/gitmeta.rs", "crates/goway/src/runners.rs", "crates/goway/tests/repo_code_never_runs.rs", "docs/usage.md", "docs/design.md"]

[[acceptance]]
text = "Given a repository whose rust-toolchain.toml points at a committed program, when ecotools::rust_min or drift::laptop_version runs, then the program never executes"
bound = true

[[acceptance]]
text = "Given a repository with core.fsmonitor and core.hooksPath set to committed programs, when goway lists or packs its files, then those programs never run"
bound = true
+++
