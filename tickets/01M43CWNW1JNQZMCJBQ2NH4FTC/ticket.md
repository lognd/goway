+++
id = "01M43CWNW1JNQZMCJBQ2NH4FTC"
title = "Learn each repository's peak memory per run and only place runs where it fits; explain an out-of-memory kill"
type = "story"
category = "in-progress"
priority = "high"
points = 3
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-04T12:05:48Z"
updated = "2026-10-04T12:43:29Z"
scope = ["crates/goway/src/footprint.rs", "crates/goway/src/pool.rs", "crates/goway/src/remote.sh", "crates/goway/src/run.rs", "crates/goway/tests/mem_footprint.rs", "docs/usage.md", "crates/goway/src/needs.rs", "crates/goway/src/status.rs"]

[[acceptance]]
text = "Given previous runs of a repository, when goway picks a host, then it knows the repository's peak memory on a helper (the job tree's peak, measured from the cgroup's memory.peak when the job runs in its own systemd scope, otherwise by sampling the process group's resident memory), never sends it to a helper whose total memory is below that peak plus a margin, and queues it (~ZW327DY) while a helper's available memory is below it; found live on 2026-10-04: two large Rust debug builds placed on a 3.6 GiB helper with --needs mem>=1500M had rustc OOM-killed"
bound = true

[[acceptance]]
text = "Given a run in which the kernel's OOM killer killed a process of the job, when goway reports the result, then it says the helper ran out of memory, the repository's measured peak and the helper's total, and suggests another host, --needs mem>=SIZE, fewer parallel jobs (CARGO_BUILD_JOBS) or giving the helper more memory (goway-setup tune on WSL helpers)"
bound = true
+++
