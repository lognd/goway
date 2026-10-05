+++
id = "01M451F9V3WBZM37PF85HN8AZK"
title = "Tests leave one sccache server per test world running, and about 80 of them (34 threads each) exhaust a goway job's 4096 task cap"
type = "bug"
category = "todo"
priority = "medium"
points = 3
reporter = "lognd"
created = "2026-10-05T03:24:44Z"
updated = "2026-10-05T03:25:07Z"
scope = ["crates/goway/src/main.rs"]

[[acceptance]]
text = "Given a many-core helper, when goway clients run concurrently, then each client's thread count does not grow with the machine's core count"
bound = false
+++

Found while landing the CI fixes. A whole-workspace nextest run in a goway job reaches the cap (ulimit -u fallback and TasksMax count threads) at about test 630: ps shows about 80 sccache servers of 34 threads each, never stopped, then fork and thread spawns fail with EAGAIN (45 to 190 failures). Related to the leftover-scope ticket for nested runs; the world teardown (or run_local sccache tests) must stop its server. Scope still to be set by whoever takes it.
