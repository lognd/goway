+++
id = "01M451D50SW5GSZDGSMZQ447N2"
title = "The default per-job task cap (4096) starves real builds and test runs: fork and thread spawn fail with EAGAIN"
type = "bug"
category = "in-progress"
priority = "critical"
points = 2
reporter = "lognd"
created = "2026-10-05T03:23:34Z"
updated = "2026-10-05T03:52:58Z"
scope = ["crates/goway/src/remote.sh", "crates/goway/src/config.rs", "crates/goway/tests/job_limits.rs", "docs/config.md", "crates/goway/src/remote.ps1", "docs/usage.md"]

[[acceptance]]
text = "Given no job_tasks configured, When a job starts, Then its task cap is derived from the host (half of kernel threads-max, and never below a documented floor well above what a full build or test wave of the host's cores uses), and the probe or run note shows the value"
bound = true

[[acceptance]]
text = "Given goway's own workspace test suite run as one job on a helper, When it runs with the default cap, Then no fork or thread spawn fails with EAGAIN"
bound = true

[[acceptance]]
text = "Given a fork bomb in a job, When it runs, Then the cap still stops it before the host's own sshd and user sessions are starved (the existing job_limits test keeps proving this)"
bound = true
+++

Reproduced on a 12-core helper: goway's own workspace test suite as one goway job hits TasksMax=4096 in its scope (threads count as tasks): dozens of 'fork: retry: Resource temporarily unavailable', 'failed to spawn thread ... WouldBlock', 'cannot run git/ssh: os error 11'. The cap from ~HNJDP23 is meant to stop a runaway job from taking the helper down, not to limit normal heavy work; the kernel's threads-max there is 62570. The no-systemd ulimit -u path has the same default.
