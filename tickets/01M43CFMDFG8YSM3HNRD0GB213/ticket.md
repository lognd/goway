+++
id = "01M43CFMDFG8YSM3HNRD0GB213"
title = "Pending claims reserve their repository's disk footprint so a wave of one repository spreads across helpers"
type = "task"
category = "in-progress"
priority = "medium"
points = 2
parent = "01M42BHPTWVNZ4DZ0H4GXG60MX"
reporter = "lognd"
created = "2026-10-04T11:58:41Z"
updated = "2026-10-04T12:25:22Z"
scope = ["crates/goway/src/queue.rs", "crates/goway/src/pool.rs", "docs/usage.md"]

[[acceptance]]
text = "Given two runs of one repository started together and a helper with room for only one more copy of its footprint, when both choose a host, then the second sees the first's pending claim subtract that footprint from the helper's free disk and picks another helper or waits"
bound = true
+++

found while working the footprint ticket: pending claims reserved memory and job slots but not disk
