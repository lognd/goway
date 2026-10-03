+++
id = "01M41F1RCK7DZQNPXF6PFDSEFK"
title = "Warm cargo runs used binaries whose baked CARGO_MANIFEST_DIR pointed at a deleted work dir"
type = "bug"
category = "todo"
priority = "high"
points = 3
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-03T18:05:03Z"
updated = "2026-10-03T18:05:03Z"
scope = ["crates/goway/src/remote.sh", "crates/goway/tests/run_local.rs", "docs/usage.md", "docs/design.md"]

[[acceptance]]
text = "Given a build in target slot k baked the absolute path of its source tree, when a later run reuses slot k without rebuilding, then that path exists and holds the current tree"
bound = false
+++

Each run had its own work dir path while sharing the cargo target dir; cargo treats a moved workspace as fresh, so binaries reused across runs referenced the previous (deleted) work dir via env!(CARGO_MANIFEST_DIR), file!() and similar. Found when frob ran goway's own suite through goway: 5 tests could not find files.
