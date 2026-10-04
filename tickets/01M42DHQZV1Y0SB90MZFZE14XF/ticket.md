+++
id = "01M42DHQZV1Y0SB90MZFZE14XF"
title = "Default max_jobs is every core of the helper (not half); niceness and owner awareness keep it polite"
type = "task"
category = "in-progress"
priority = "medium"
points = 1
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-04T02:58:04Z"
updated = "2026-10-04T03:54:29Z"
scope = ["crates/goway/src/config.rs", "crates/goway/src/pool.rs", "docs/config.md"]

[[acceptance]]
text = "Given a [[host]] without max_jobs, when goway counts its job limit, then the limit is the helper's core count (at least 1), as the owner asked on 2026-10-03; docs explain that jobs run at nice 10 (nice 19 and half the cores while the owner is using the helper) and how to lower max_jobs"
bound = true
+++
