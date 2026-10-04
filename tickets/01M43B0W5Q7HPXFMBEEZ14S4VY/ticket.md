+++
id = "01M43B0W5Q7HPXFMBEEZ14S4VY"
title = "A per-repository sccache server inherits one run's TMPDIR and breaks later runs after that work dir is removed"
type = "bug"
category = "todo"
priority = "high"
points = 2
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-04T11:33:08Z"
updated = "2026-10-04T11:51:36Z"
scope = ["crates/goway/src/remote.sh", "crates/goway/tests/sccache_tmpdir.rs", "docs/design.md"]

[[acceptance]]
text = "Given two consecutive runs of one repository on a helper (the first starts the shared sccache server, then its work dir and TMPDIR are removed), when the second run compiles, then sccache works: the server is started (or restarted) with a TMPDIR that outlives runs (for example the repository cache's own tmp dir), never a run's work dir; found live: 'sccache: Failed to create temp dir' pointing at a deleted goway work dir"
bound = true

[[acceptance]]
text = "Given a test that runs cargo twice through goway with sccache present, when the first run's work dir is gone, then the second build succeeds"
bound = true
+++
