+++
id = "01M44VAJE0Z7NQ4YQGK1ABKWDY"
title = "Queue arrival-order test is too tight for a slow macOS runner"
type = "bug"
category = "in-progress"
priority = "medium"
points = 2
reporter = "lognd"
created = "2026-10-05T01:37:18Z"
updated = "2026-10-05T02:21:56Z"
scope = ["crates/goway/tests/queue.rs"]

[[acceptance]]
text = "Given a slow loaded runner, when twenty runs arrive over three hosts, then each is served in arrival order within the stated tolerance without relying on thread wake-up jitter"
bound = true
+++

CI run 37251466869 (macos job): twenty_runs_spread_over_three_hosts_in_arrival_order... saw 15 served at place 12 (tolerance 2). Timing-sensitive sleeps of 60 ms per arrival.
