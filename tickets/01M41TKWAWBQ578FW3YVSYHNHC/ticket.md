+++
id = "01M41TKWAWBQ578FW3YVSYHNHC"
title = "Detect GoogleTest and Catch2 test binaries by their embedded flag strings on the helper, so --shard needs no GOWAY_RUNNER"
type = "story"
category = "in-progress"
priority = "medium"
points = 3
parent = "01M41P2FPBSV4WFSQDGQY7SC89"
reporter = "lognd"
created = "2026-10-03T21:27:11Z"
updated = "2026-10-03T21:47:52Z"
scope = ["crates/goway/src/runners.rs", "crates/goway/src/shard.rs", "crates/goway/src/remote.sh", "crates/goway/tests/**", "docs/usage.md", "crates/goway/src/detect.rs", "crates/goway/src/run.rs", "crates/goway/src/gc.rs", "crates/goway/src/state.rs", "crates/goway/src/lib.rs"]

[[acceptance]]
text = "Given goway run --shard N -- ./build/tests where the binary is built on the helper, when the shard starts, then the helper reads (never executes) the file: GoogleTest markers (GTEST_SHARD_INDEX, --gtest_list_tests) set GTEST_TOTAL_SHARDS and GTEST_SHARD_INDEX; Catch2 v3 markers (--shard-count and --shard-index) add those flags; anything else runs unchanged with GOWAY_SHARD and GOWAY_SHARD_COUNT"
bound = true

[[acceptance]]
text = "Given stripped release binaries of GoogleTest and Catch2 v3 (and a Catch2 v2 binary without sharding flags), when detected, then the first two are sharded natively and the v2 binary falls back to GOWAY_SHARD, proven by tests with real or faithful fixture binaries"
bound = true

[[acceptance]]
text = "Given GOWAY_RUNNER=catch2 or gtest, when set, then it overrides detection"
bound = true

[[acceptance]]
text = "Given a candidate program, when goway inspects it, then it is only treated as GoogleTest or Catch2 if it is an ELF or PE executable (magic bytes, never scripts) containing every required marker (GoogleTest: GTEST_SHARD_INDEX, GTEST_TOTAL_SHARDS, --gtest_list_tests, --gtest_filter; Catch2 v3: --shard-count, --shard-index, --list-tests and a Catch2-specific string), read with a size cap and fixed-string search, resolved exactly as the shell will resolve it, and never executed to find out; goway's own binary and fixtures that contain only some markers are not detected"
bound = true

[[acceptance]]
text = "Given a GoogleTest shard, when it runs, then GOWAY sets GTEST_SHARD_STATUS_FILE and afterwards treats a missing status file as 'sharding not applied': a warning names the program, no rerun happens (each shard ran the whole suite, so results stand), and the report notes the duplicated work"
bound = true

[[acceptance]]
text = "Given a Catch2 shard rejected with Catch2's unknown-option error before any test ran, when it ends, then goway warns, reruns that shard once without the shard flags, and records both attempts; any less certain failure is flagged and never rerun"
bound = true

[[acceptance]]
text = "Given a failed detection, when later runs of the same program path in the same repository are sharded, then goway uses the GOWAY_SHARD fallback with a short note, from a marker kept only in local state; GOWAY_RUNNER=gtest|catch2 overrides it and goway gc --repo clears it"
bound = true

[[acceptance]]
text = "Given a Catch2 shard that is rejected again after its rerun without shard flags, when it ends, then goway stops (that rerun is the only one, enforced by an explicit attempt argument, never by environment or helper output) and reports the failure; a test with a binary that always rejects proves exactly one rerun"
bound = false
+++
