+++
id = "01M548GAQ2DM0A38BZPWF3BY7Y"
title = "Triage ~20 failing goway tests on main (run_local, slot_trees, client_loss, cmake_api, clock, copy_integrity) and 84 exec-failures"
type = "bug"
category = "in-progress"
priority = "medium"
points = 3
reporter = "lognd"
created = "2026-10-17T06:24:05Z"
updated = "2026-10-17T08:20:51Z"
scope = ["crates/goway/tests/common/mod.rs", "crates/goway/tests/local_host.rs", "crates/goway/tests/slot_wait.rs", "crates/goway/tests/weighted_shards.rs", "changelog.d/01M548GAQ2DM0A38BZPWF3BY7Y*"]

[[acceptance]]
text = "Formerly failing run_local, slot_trees, clock, copy_integrity, cmake_api, weighted_shards, local_host, slot_wait test binaries pass on a host with little free disk"
bound = true
+++

Phase agents report 19-21 failures plus 84 exec-failed in the full -p goway -p goway-circle run. run_local also fails on a clean main. Unconfirmed whether it is environment (WSL, load, missing cmake) or real. Re-run on main, separate environment from real failures, fix or gate.
