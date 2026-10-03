+++
id = "01M41P2FWVAT5AP3XD121HE9YF"
title = "Shard adapters for common test frameworks"
type = "story"
category = "in-progress"
priority = "medium"
points = 8
parent = "01M41P2FPBSV4WFSQDGQY7SC89"
reporter = "lognd"
created = "2026-10-03T20:07:47Z"
updated = "2026-10-03T21:24:53Z"
scope = ["crates/goway/src/shard.rs", "crates/goway/src/runners.rs", "crates/goway/src/lib.rs", "crates/goway/tests/shard_*.rs", "docs/usage.md", "crates/goway/src/render.rs"]

[[acceptance]]
text = "Given goway run --shard N with vitest, jest, Playwright, Catch2, GoogleTest, CTest or cargo-nextest, when it runs, then each shard uses the framework's own sharding and together they run every test exactly once"
bound = true

[[acceptance]]
text = "Given goway run --shard N with pytest, go test, Maven, Gradle or RSpec, when it runs, then goway splits the test files, packages or classes deterministically across shards and together they cover every test exactly once"
bound = false

[[acceptance]]
text = "Given an unknown command, when it is sharded, then it gets GOWAY_SHARD and GOWAY_SHARD_COUNT and runs unchanged"
bound = false
+++
