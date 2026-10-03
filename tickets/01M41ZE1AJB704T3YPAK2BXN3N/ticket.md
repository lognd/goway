+++
id = "01M41ZE1AJB704T3YPAK2BXN3N"
title = "Shares note lists hosts in a random order, so the weighted sharding test is flaky"
type = "bug"
category = "todo"
priority = "high"
points = 1
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-03T22:51:22Z"
updated = "2026-10-03T22:51:28Z"
scope = ["crates/goway/src/shard.rs", "crates/goway/src/runners.rs"]

[[acceptance]]
text = "Given a weighted sharded run, when goway prints the shares note, then hosts are listed by share, largest first, then by name, so output and tests are deterministic"
bound = false
+++
