+++
id = "01M4219T92MBXK35BWZ5MMJ7N2"
title = "Apply HostConfig::job_limit in the pool and status (default max_jobs = cores/2)"
type = "task"
category = "todo"
priority = "medium"
points = 1
reporter = "lognd"
created = "2026-10-03T23:24:01Z"
updated = "2026-10-03T23:53:07Z"
scope = ["crates/goway/src/pool.rs", "crates/goway/src/status.rs"]

[[acceptance]]
text = "Given a host with max_jobs unset and 8 cores, when it already runs 4 goway jobs, then the pool skips it and status shows 4/4"
bound = false
+++

Follow-up of YNB953N: HostConfig::job_limit exists in config.rs; the pool and status wiring was out of that ticket's scope.
