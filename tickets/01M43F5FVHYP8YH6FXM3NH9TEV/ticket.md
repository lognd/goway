+++
id = "01M43F5FVHYP8YH6FXM3NH9TEV"
title = "The clock-jump gc test fails on a host that booted under two minutes ago"
type = "bug"
category = "done"
outcome = "done"
priority = "medium"
points = 1
reporter = "lognd"
created = "2026-10-04T12:45:34Z"
updated = "2026-10-04T12:50:34Z"
scope = ["crates/goway/tests/clock.rs"]

[[acceptance]]
text = "Given a helper whose uptime is under the work-dir grace, when the clock-jump gc test runs, then it still proves a starting run survives and never fails because its long-ago dir is still inside the grace by uptime"
bound = true
+++
