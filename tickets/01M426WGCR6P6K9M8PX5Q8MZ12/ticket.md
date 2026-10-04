+++
id = "01M426WGCR6P6K9M8PX5Q8MZ12"
title = "Test world: pin max_jobs so tests with concurrent runs do not hit the cores/2 default on small CI runners"
type = "bug"
category = "in-progress"
priority = "high"
points = 1
reporter = "lognd"
created = "2026-10-04T01:01:37Z"
updated = "2026-10-04T01:09:48Z"
scope = ["crates/goway/tests/common/mod.rs"]

[[acceptance]]
text = "Given a CI runner with 4 cores, when nested and concurrent-run tests execute, then the test host is not skipped for its job limit"
bound = true
+++

found while working ~FKHDK2A: main CI run 37166058824 fails nesting::recursive_goway_stops_at_the_depth_limit and gpu_slots tests since ~5MMJ7N2 made max_jobs default to cores/2.
