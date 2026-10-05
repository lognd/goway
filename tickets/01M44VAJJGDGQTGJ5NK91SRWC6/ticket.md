+++
id = "01M44VAJJGDGQTGJ5NK91SRWC6"
title = "Making room never evicts the run's own seed: fails on the macOS runner"
type = "bug"
category = "todo"
priority = "medium"
points = 3
reporter = "lognd"
created = "2026-10-05T01:37:18Z"
updated = "2026-10-05T01:37:18Z"
scope = ["crates/goway/tests/footprint.rs"]

[[acceptance]]
text = "Given macOS, when a run makes room on a nearly full disk, then the seed it was synced from is still present afterwards"
bound = false
+++

CI run 37251466869 (macos job): making_room_never_evicts_the_runs_own_seed found no seed after the second run though the run reported freeing 0 B. Root cause to be found.
