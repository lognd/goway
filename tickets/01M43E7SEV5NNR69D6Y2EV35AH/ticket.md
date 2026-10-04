+++
id = "01M43E7SEV5NNR69D6Y2EV35AH"
title = "weighted_shards tests fail under a loaded helper: weights come from live load, not the faked core counts"
type = "bug"
category = "todo"
priority = "medium"
points = 1
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-04T12:29:21Z"
updated = "2026-10-04T12:29:21Z"
scope = ["crates/goway/tests/weighted_shards.rs"]

[[acceptance]]
text = "Given a helper under heavy load, when the weighted_shards tests run, then they pass"
bound = false
+++

found while working ~5WD6GZ8: both tests fail in the full suite on a helper with load 36+ on 12 cores (expected 4:1 partitions, got 1:1; the vitest test failed too). Pin the load in the fake probe or compare capacity only.
