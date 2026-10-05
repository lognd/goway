+++
id = "01M44VAJ9FGTQSC9KGPCBV8T5J"
title = "Job-limit tests assume systemd and a high process hard limit, which macOS has neither"
type = "bug"
category = "in-progress"
priority = "medium"
points = 2
reporter = "lognd"
created = "2026-10-05T01:37:18Z"
updated = "2026-10-05T01:37:21Z"
scope = ["crates/goway/tests/job_limits.rs"]

[[acceptance]]
text = "Given macOS, when the job-limit tests run, then the systemd scope test is skipped with a stated reason and the ulimit test expects the cap clamped to the platform hard limit"
bound = false
+++

CI run 37251466869 (macos job): a_job_scope_carries_... reads a systemd-run log that macOS never writes (no systemd; remote.sh skips the scope there by design); without_a_user_manager_... expects ulimit -u >= 6000 but the macOS runner's hard process limit is 1333 so the ulimit fallback cannot raise it.
