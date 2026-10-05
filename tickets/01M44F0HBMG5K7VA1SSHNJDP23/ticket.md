+++
id = "01M44F0HBMG5K7VA1SSHNJDP23"
title = "One job cannot take a helper down: cap a job's processes, CPU share and memory in its own scope"
type = "story"
category = "done"
outcome = "done"
priority = "high"
points = 3
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-04T22:02:06Z"
updated = "2026-10-05T01:15:50Z"
scope = ["crates/goway/src/remote.sh", "crates/goway/src/config.rs", "crates/goway/tests/job_limits.rs", "docs/config.md", "docs/usage.md", "crates/goway/src/run.rs", "crates/goway/src/remote.ps1"]

[[acceptance]]
text = "Given a job on a Linux helper with systemd user scopes, when it starts, then it runs in a scope with TasksMax (default 4096), CPUWeight low and an optional CPUQuota and MemoryMax (config job_tasks, job_cpu, job_memory, per host), so a runaway job (found live: a stress loop of 604 processes drove a 12-core helper to load 620) is contained; without systemd, ulimit -u and nice still apply; limits are documented and overridable"
bound = true
+++
