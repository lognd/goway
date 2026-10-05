+++
id = "01M44YARTF6NAN21B2WYT0QS1A"
title = "The job-scope test assumes cgroup v2, which a hybrid-cgroup Linux machine lacks"
type = "bug"
category = "todo"
priority = "medium"
points = 1
reporter = "lognd"
created = "2026-10-05T02:29:50Z"
updated = "2026-10-05T02:29:50Z"
scope = ["crates/goway/tests/job_limits.rs"]

[[acceptance]]
text = "Given a Linux machine without cgroup v2, when the job-scope test runs, then it is skipped with a stated reason instead of failing"
bound = false
+++

Found running the full suite on a helper whose /sys/fs/cgroup has no cgroup.controllers (cgroup v1 hybrid): remote.sh skips the scope there by design, so the fake systemd-run is never called. The test must skip, saying why, where cgroup v2 is absent.
