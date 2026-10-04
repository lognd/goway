+++
id = "01M43CZG0V20SHCRVJ75YX9H8Q"
title = "Making room before a run must never evict the run's own seed, work dir or cache"
type = "bug"
category = "in-progress"
priority = "high"
points = 2
parent = "01M42BHPTWVNZ4DZ0H4GXG60MX"
reporter = "lognd"
created = "2026-10-04T12:07:20Z"
updated = "2026-10-04T12:23:25Z"
scope = ["crates/goway/src/remote.sh", "crates/goway/tests/footprint.rs"]

[[acceptance]]
text = "Given a helper short of room whose only evictable entry is the run's own seed, when the run starts, then the seed, work dir and cache are kept and the run proceeds"
bound = true
+++

found by the coordinator verifying the footprint ticket: the pre-run eviction removed the seed the run had just synced
