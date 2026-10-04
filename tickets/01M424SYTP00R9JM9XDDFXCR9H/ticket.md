+++
id = "01M424SYTP00R9JM9XDDFXCR9H"
title = "A remote job outlives a client that vanished without a hangup; GPU-slot tests leak busy pollers when they fail"
type = "bug"
category = "in-progress"
priority = "high"
points = 3
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-04T00:25:16Z"
updated = "2026-10-04T00:35:43Z"
scope = ["crates/goway/tests/gpu_slots.rs", "crates/goway/tests/common/mod.rs"]

[[acceptance]]
text = "Given the GPU-slot and other tests that hold a run open until a release file appears, when such a test fails, panics or is cancelled, then a drop guard releases or kills what it started, so no poller survives the test (on 2026-10-03 96 orphaned pollers kept a helper at 3 cores for 20 minutes)"
bound = true
+++
