+++
id = "01M41TKWAWBQ578FW3YVSYHNHC"
title = "Detect GoogleTest and Catch2 test binaries by their embedded flag strings on the helper, so --shard needs no GOWAY_RUNNER"
type = "story"
category = "todo"
priority = "medium"
points = 2
parent = "01M41P2FPBSV4WFSQDGQY7SC89"
reporter = "lognd"
created = "2026-10-03T21:27:11Z"
updated = "2026-10-03T21:27:11Z"
scope = ["crates/goway/src/runners.rs", "crates/goway/src/shard.rs", "crates/goway/src/remote.sh", "crates/goway/tests/**", "docs/usage.md"]

[[acceptance]]
text = "Given goway run --shard N -- ./build/tests where the binary is built on the helper, when the shard starts, then the helper reads (never executes) the file: GoogleTest markers (GTEST_SHARD_INDEX, --gtest_list_tests) set GTEST_TOTAL_SHARDS and GTEST_SHARD_INDEX; Catch2 v3 markers (--shard-count and --shard-index) add those flags; anything else runs unchanged with GOWAY_SHARD and GOWAY_SHARD_COUNT"
bound = false

[[acceptance]]
text = "Given stripped release binaries of GoogleTest and Catch2 v3 (and a Catch2 v2 binary without sharding flags), when detected, then the first two are sharded natively and the v2 binary falls back to GOWAY_SHARD, proven by tests with real or faithful fixture binaries"
bound = false

[[acceptance]]
text = "Given GOWAY_RUNNER=catch2 or gtest, when set, then it overrides detection"
bound = false
+++
