+++
id = "01M55SHEYAHRMNZV0VWTQJRXPS"
title = "Audit M3: removing a lead role does not bump the epoch"
type = "security"
category = "in-progress"
priority = "medium"
points = 3
parent = "01M54433AG8J1V0GYT3ZWY9C9A"
reporter = "lognd"
created = "2026-10-17T20:41:03Z"
updated = "2026-10-17T20:54:58Z"
idempotency_key = "audit-m3"
scope = ["crates/goway-circle/src/state.rs", "crates/goway-circle/tests/roster.rs", "docs/circles.md", "docs/model/circle_model.py", "docs/model/test_circle_model.py", "changelog.d/01M55SHEYAHRMNZV0VWTQJRXPS.fixed.md"]

[[acceptance]]
text = "Given set_roles removes a lead role, when the roster is applied, then the epoch advances and old certs stop validating"
bound = true
+++

Bump epoch on role removal.
