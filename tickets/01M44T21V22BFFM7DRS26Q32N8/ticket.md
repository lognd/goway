+++
id = "01M44T21V22BFFM7DRS26Q32N8"
title = "Slot-wait test fails on small CI runners: the local test host is held back by the per-job memory admission"
type = "bug"
category = "todo"
priority = "medium"
points = 1
reporter = "lognd"
created = "2026-10-05T01:15:10Z"
updated = "2026-10-05T01:15:10Z"
scope = ["crates/goway/tests/slot_wait.rs"]

[[acceptance]]
text = "Given a runner with little free memory, when the slot-wait test holds both build slots and runs with --wait 2s, then it reaches the helper-side slot wait and exits 125 naming the busy slots"
bound = false
+++

CI run 37250187958: client admission (job_mem 1.5G) reported no host has room; the test config should turn the memory test off (job_mem = 0).
