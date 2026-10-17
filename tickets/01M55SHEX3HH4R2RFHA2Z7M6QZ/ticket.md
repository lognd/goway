+++
id = "01M55SHEX3HH4R2RFHA2Z7M6QZ"
title = "Audit M2: roster read-merge-write is not locked, concurrent sync can lose a revoke"
type = "security"
category = "done"
outcome = "done"
priority = "high"
points = 3
parent = "01M54433AG8J1V0GYT3ZWY9C9A"
reporter = "lognd"
created = "2026-10-17T20:41:03Z"
updated = "2026-10-17T20:51:28Z"
idempotency_key = "audit-m2"
scope = ["crates/goway-circle/src/store.rs,crates/goway/src/circle.rs", "crates/goway-circle/src/store.rs", "crates/goway/src/circle.rs", "crates/goway-circle/src/lib.rs"]

[[acceptance]]
text = "Given two concurrent syncs, when both merge, then no operation is lost"
bound = true
+++

Take an exclusive file lock around read-merge-write.
