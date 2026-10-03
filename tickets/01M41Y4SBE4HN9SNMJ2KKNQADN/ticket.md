+++
id = "01M41Y4SBE4HN9SNMJ2KKNQADN"
title = "Real Catch2 v3 binaries are not detected: the marker Catch2TestRun does not exist in them"
type = "bug"
category = "todo"
priority = "high"
points = 2
parent = "01M41P2FPBSV4WFSQDGQY7SC89"
reporter = "lognd"
created = "2026-10-03T22:28:51Z"
updated = "2026-10-03T22:28:51Z"
scope = ["crates/goway/src/remote.sh", "crates/goway/tests/**", ".github/workflows/ci.yml", "docs/usage.md"]

[[acceptance]]
text = "Given a stripped Catch2 v3.7.1 binary built with CMake FetchContent, when goway run --shard 2 runs it, then it is detected (markers --shard-count, --shard-index, --list-tests, catch2-version, all present in the real binary) and the two shards run disjoint halves of its test cases"
bound = false

[[acceptance]]
text = "Given CI on Linux, when it runs, then a job builds real GoogleTest v1.15.2 and Catch2 v3.7.1 test binaries through CMake FetchContent, strips them, and runs goway's detection and sharding against them, so fixtures can never drift from the real frameworks again"
bound = false
+++
