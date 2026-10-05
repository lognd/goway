+++
id = "01M451F9V3WBZM37PF85HN8AZK"
title = "Tests leave one sccache server per test world running, and about 80 of them (34 threads each) exhaust a goway job's 4096 task cap"
type = "bug"
category = "done"
outcome = "done"
priority = "medium"
points = 3
reporter = "lognd"
created = "2026-10-05T03:24:44Z"
updated = "2026-10-05T03:38:36Z"
scope = ["crates/goway/tests/common/mod.rs", "crates/goway/tests/world_teardown.rs"]

[[acceptance]]
text = "Given a test world that ran goway with sccache installed, When the world is dropped, Then no sccache server whose SCCACHE_DIR lies under the world remains"
bound = true
+++

Found while landing the CI fixes. A whole-workspace nextest run in a goway job reaches the cap (ulimit -u fallback and TasksMax count threads) at about test 630: ps shows about 80 sccache servers of 34 threads each, never stopped, then fork and thread spawns fail with EAGAIN (45 to 190 failures). Related to the leftover-scope ticket for nested runs; the world teardown (or run_local sccache tests) must stop its server. Scope still to be set by whoever takes it.
