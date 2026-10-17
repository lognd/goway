+++
id = "01M5443BNG19E2ZT3TZ2ZV2BCK"
title = "Phase 1: goway-circle crate (ops, signing, fold, merge, anti-rollback)"
type = "story"
category = "in-progress"
priority = "high"
points = 8
parent = "01M54433AG8J1V0GYT3ZWY9C9A"
reporter = "lognd"
created = "2026-10-17T05:07:06Z"
updated = "2026-10-17T05:35:06Z"
idempotency_key = "c1"
labels = ["circles"]
scope = ["crates/goway-circle/**", "docs/circles.md", "docs/model/**", "Cargo.lock"]

[[acceptance]]
text = "Every model test has a Rust equivalent"
bound = true

[[acceptance]]
text = "Convergence and no-unauthorized-op property tests pass"
bound = true

[[acceptance]]
text = "Given the roster rules in docs/model, when the Rust crate folds the same ops, then every model test has a passing Rust equivalent"
bound = true

[[acceptance]]
text = "Given random op orderings or unauthorized ops, when folded, then state converges and unauthorized ops change nothing"
bound = true
+++

Port docs/model/circle_model.py to Rust. ssh-keygen -Y signing, JCS encoding, strict parse, size caps, property tests matching docs/model/test_circle_model.py.
