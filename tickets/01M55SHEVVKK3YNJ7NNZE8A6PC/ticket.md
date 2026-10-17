+++
id = "01M55SHEVVKK3YNJ7NNZE8A6PC"
title = "Audit M1: anti-rollback checks head ids only and join replaces the roster"
type = "security"
category = "in-progress"
priority = "high"
points = 3
parent = "01M54433AG8J1V0GYT3ZWY9C9A"
reporter = "lognd"
created = "2026-10-17T20:41:03Z"
updated = "2026-10-17T20:53:35Z"
idempotency_key = "audit-m1"
scope = ["crates/goway-circle/src/state.rs,crates/goway-circle/src/store.rs,crates/goway/src/circle.rs", "crates/goway-circle/src/store.rs", "crates/goway-circle/src/lib.rs", "docs/circles.md", "docs/model/circle_model.py", "docs/model/test_circle_model.py", "changelog.d/01M55SHEVVKK3YNJ7NNZE8A6PC.fixed.md"]

[[acceptance]]
text = "Given a crafted welcome that drops a revoke, when join runs on a stored roster, then it is refused"
bound = true
+++

Compare ancestry not head ids; join must not replace a stored roster with one lacking its ops.
