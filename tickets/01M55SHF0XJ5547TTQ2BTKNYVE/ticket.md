+++
id = "01M55SHF0XJ5547TTQ2BTKNYVE"
title = "Audit M7: admin cosign signs without showing what it approves"
type = "security"
category = "in-progress"
priority = "medium"
points = 3
parent = "01M54433AG8J1V0GYT3ZWY9C9A"
reporter = "lognd"
created = "2026-10-17T20:41:03Z"
updated = "2026-10-17T20:58:11Z"
idempotency_key = "audit-m7"
scope = ["crates/goway/src/circle_admin.rs", "crates/goway/src/cli.rs", "crates/goway/tests/circle_admin.rs", "docs/circles.md"]

[[acceptance]]
text = "Given admin cosign, when run, then the operation is shown and confirmed before signing"
bound = true
+++

Render the op and require confirmation (or --yes).
