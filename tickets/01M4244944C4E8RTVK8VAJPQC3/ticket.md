+++
id = "01M4244944C4E8RTVK8VAJPQC3"
title = "Clock safety: measure helper clock offset, never let a clock jump expose a starting run to gc, keep builds warm when the laptop clock runs ahead"
type = "bug"
category = "in-progress"
priority = "high"
points = 3
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-04T00:13:26Z"
updated = "2026-10-04T06:11:07Z"
scope = ["crates/goway/src/remote.sh", "crates/goway/tests/slot_trees.rs", "crates/goway/tests/clock.rs", "docs/design.md", "docs/troubleshooting.md"]

[[acceptance]]
text = "Given a run that has just created its work directory, when the helper's wall clock jumps forward by any amount, then gc never removes it: a fresh work directory is protected by a liveness check (its creator process is alive on that helper), not only by its age; a test simulates the jump"
bound = true

[[acceptance]]
text = "Given synced files whose mtimes are in the helper's future (the laptop clock runs ahead), when the slot is reconciled, then their slot copies are clamped to the helper's current time so make and ninja neither warn about clock skew nor rebuild on every run, tar's future-timestamp warnings are suppressed, and a test with mtimes one hour ahead proves the second run rebuilds nothing"
bound = true

[[acceptance]]
text = "Given docs/design.md, when read, then it states that goway stores only epoch seconds (timezones never matter), which clock each comparison uses (laptop or helper), and why no comparison mixes the two"
bound = true
+++
