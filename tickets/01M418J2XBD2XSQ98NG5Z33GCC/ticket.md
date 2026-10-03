+++
id = "01M418J2XBD2XSQ98NG5Z33GCC"
title = "Pool scheduling: parallel probes and least-loaded pick"
type = "task"
category = "todo"
priority = "medium"
points = 3
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-03T16:11:38Z"
updated = "2026-10-03T16:11:38Z"
scope = ["crates/goway/**", "docs/**"]

[[acceptance]]
text = "Given probes of several hosts, when goway picks one, then it chooses the lowest (load1 + goway jobs) / cores among reachable hosts under max_jobs"
bound = false
+++
