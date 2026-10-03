+++
id = "01M41R6XC8FGG8NC36TYZ9GC8F"
title = "Output integrity: one write per line under one lock shared by stdout, stderr and goway's own messages; bounded, batched line reads"
type = "bug"
category = "in-progress"
priority = "medium"
points = 3
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-03T20:45:09Z"
updated = "2026-10-03T21:12:59Z"
scope = ["crates/goway/src/render.rs", "crates/goway/src/shard.rs", "crates/goway/src/termfilter.rs", "crates/goway/src/run.rs", "crates/goway/tests/**", "docs/usage.md"]

[[acceptance]]
text = "Given a sharded run where many hosts stream stdout and stderr to the same terminal or pipe, when lines arrive concurrently, then every output line is written by one write call under one lock shared by both streams, so no line is ever split or interleaved with another (a stress test with many writer threads checks every line arrives whole)"
bound = false

[[acceptance]]
text = "Given goway's own messages (info, note, warning, error) printed while remote output streams, when both happen at once, then goway's message is assembled into one buffer and written whole under the same lock"
bound = false

[[acceptance]]
text = "Given a host whose output ends without a newline, or a line longer than the cap (64 KiB), when it is relayed with a prefix, then the partial line is terminated (or split with a continuation prefix) so the next host's line never joins it, and memory per stream stays bounded"
bound = false

[[acceptance]]
text = "Given heavy output, when lines are relayed, then all complete lines already buffered are written in one batch per lock, and a slow terminal applies back-pressure to the remote command instead of growing memory"
bound = false
+++
