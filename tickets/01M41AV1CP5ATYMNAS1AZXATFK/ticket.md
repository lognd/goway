+++
id = "01M41AV1CP5ATYMNAS1AZXATFK"
title = "Good-neighbor execution: low priority, load ceiling and owner-activity awareness on shared hosts"
type = "task"
category = "in-progress"
priority = "medium"
points = 3
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-03T16:51:28Z"
updated = "2026-10-03T17:09:40Z"
scope = ["crates/goway/**", "docs/config.md", "docs/positioning.md", "docs/usage.md"]

[[acceptance]]
text = "Given a host config with priority = low, when goway runs a job, then the job runs under nice 10 and idle-class I/O"
bound = true

[[acceptance]]
text = "Given a host whose load per core is above its max_load, when goway picks a host, then that host is skipped unless pinned with --host"
bound = true
+++
