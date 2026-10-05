+++
id = "01M44F0H5R09BNX6PJWD2QQV2Q"
title = "The helper-side wait for a build slot ignores --wait; bound it and say who holds the slots"
type = "bug"
category = "in-progress"
priority = "high"
points = 2
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-04T22:02:06Z"
updated = "2026-10-05T00:37:14Z"
scope = ["crates/goway/src/remote.ps1", "crates/goway/src/run.rs", "crates/goway/tests/slot_wait.rs", "docs/usage.md", "crates/goway/src/shard.rs", "crates/goway/src/remote.sh"]

[[acceptance]]
text = "Given all of a repository's build slots busy on the chosen helper, when a run waits for one, then the wait is bounded by the same --wait (default 5 minutes), the note says how many slots are busy and how long the oldest holder has run, and on timeout goway exits 125 saying so (found live: a run waited 40 minutes for a slot)"
bound = true
+++
