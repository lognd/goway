+++
id = "01M41SDZPWESK72BNQ03Y6T2FJ"
title = "Capacity-weighted sharding: bigger hosts take a bigger share of the tests"
type = "story"
category = "todo"
priority = "medium"
points = 3
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-03T21:06:29Z"
updated = "2026-10-03T21:06:29Z"
scope = ["crates/goway/src/shard.rs", "crates/goway/src/runners.rs", "crates/goway/src/pool.rs", "crates/goway/tests/**", "docs/usage.md"]

[[acceptance]]
text = "Given hosts with different free capacity (cores minus load), when a run is sharded, then each host receives a share of the tests proportional to its capacity (nextest and splitting adapters via weighted partitions), and together the shards still run every test exactly once"
bound = false

[[acceptance]]
text = "Given a sharded run, when it reports, then each shard's host, architecture and share are listed"
bound = false
+++
