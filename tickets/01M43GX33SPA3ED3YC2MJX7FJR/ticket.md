+++
id = "01M43GX33SPA3ED3YC2MJX7FJR"
title = "The creator-pid gc test fails on a host that booted under two minutes ago"
type = "bug"
category = "done"
outcome = "done"
priority = "medium"
points = 1
reporter = "lognd"
created = "2026-10-04T13:15:56Z"
updated = "2026-10-04T13:21:15Z"
scope = ["crates/goway/tests/clock.rs"]

[[acceptance]]
text = "Given a helper whose uptime is under the work-dir grace, when the dead-creator gc test runs, then a dir with no birth marker and a dead creator is removed and the test passes"
bound = true
+++
