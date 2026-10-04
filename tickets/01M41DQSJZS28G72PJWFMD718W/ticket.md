+++
id = "01M41DQSJZS28G72PJWFMD718W"
title = "Owner awareness: a helper whose owner is using it stays available but goway runs extra nicely there"
type = "task"
category = "in-progress"
priority = "medium"
points = 3
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-03T17:42:08Z"
updated = "2026-10-04T04:16:37Z"
scope = ["crates/goway/src/pool.rs", "crates/goway/src/config.rs", "crates/goway/src/status.rs", "crates/goway/src/remote.sh", "crates/goway/tests/owner_awareness.rs", "crates/goway/src/facts.rs", "docs/config.md", "crates/goway/src/needs.rs", "crates/goway/src/run.rs", "crates/goway/src/shard.rs"]

[[acceptance]]
text = "Given a helper whose Windows user was active within the idle window (default 5 minutes) or which runs on battery, when goway picks hosts, then the helper is never skipped for that reason; an idle helper on mains power is preferred when the choice is otherwise close (a scoring penalty, not an exclusion)"
bound = true

[[acceptance]]
text = "Given a run that lands on a helper in use, when the job starts, then it runs extra nicely: nice 19 and idle I/O class (instead of the default nice 10), and at most half the helper's cores for build parallelism (CARGO_BUILD_JOBS, MAKEFLAGS -j, CMAKE_BUILD_PARALLEL_LEVEL, NEXTEST_TEST_THREADS) unless the user set them; goway says so in one line"
bound = true

[[acceptance]]
text = "Given a helper where activity or power cannot be read (interop disabled, macOS without the tool, a probe timeout), when goway decides, then it treats the state as unknown, which neither blocks nor penalises, and status shows it"
bound = true
+++
