+++
id = "01M5443BNG19E2ZT3TZ2ZV2BCK"
title = "Phase 1: goway-circle crate (ops, signing, fold, merge, anti-rollback)"
type = "story"
category = "todo"
priority = "high"
parent = "01M54433AG8J1V0GYT3ZWY9C9A"
reporter = "lognd"
created = "2026-10-17T05:07:06Z"
updated = "2026-10-17T05:07:06Z"
idempotency_key = "c1"
labels = ["circles"]

[[acceptance]]
text = "Every model test has a Rust equivalent"
bound = false

[[acceptance]]
text = "Convergence and no-unauthorized-op property tests pass"
bound = false
+++

Port docs/model/circle_model.py to Rust. ssh-keygen -Y signing, JCS encoding, strict parse, size caps, property tests matching docs/model/test_circle_model.py.
