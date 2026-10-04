+++
id = "01M43S56WE4YZD6YZMF61GZVB4"
title = "disk_budget eviction_keeps_a_locked_slot test leaks flock's sleep child holding the test's pipes (nextest LEAK)"
type = "bug"
category = "in-progress"
priority = "medium"
points = 1
reporter = "lognd"
created = "2026-10-04T15:40:11Z"
updated = "2026-10-04T22:01:42Z"
scope = ["crates/goway/tests/disk_budget.rs"]

[[acceptance]]
text = "Given the test finishes, When nextest checks the test's handles, Then no sleep child outlives it (no LEAK)"
bound = true
+++

found while working E09XD8K: holder.kill() kills flock but not the sleep 30 it forked, which holds stdout/stderr for 30s; nextest reports LEAK on macOS. Kill the process group or exec the sleep without a fork.
