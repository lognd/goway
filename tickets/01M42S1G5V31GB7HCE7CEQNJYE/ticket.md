+++
id = "01M42S1G5V31GB7HCE7CEQNJYE"
title = "slot_trees cargo stale-build regression test fails on a helper"
type = "bug"
category = "in-progress"
priority = "medium"
points = 2
reporter = "lognd"
created = "2026-10-04T06:18:55Z"
updated = "2026-10-04T11:47:34Z"
scope = ["crates/goway/tests/slot_trees.rs", "crates/goway/src/remote.sh", "crates/goway/tests/run_local.rs", "docs/config.md", "CHANGELOG.md", "crates/goway/tests/common/mod.rs", "crates/goway/tests/cmake_api.rs"]

[[acceptance]]
text = "Given a helper with cargo, When cargo_rebuilds_when_an_older_branch_reuses_the_slot runs, Then it passes 10 of 10 times"
bound = true
+++

Fails with exit 2 on one helper; the helper hides the child's stdout so the cause is unseen.
