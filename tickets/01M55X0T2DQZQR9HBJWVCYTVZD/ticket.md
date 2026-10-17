+++
id = "01M55X0T2DQZQR9HBJWVCYTVZD"
title = "Audit L1+L9: roster stores unverified ops; undo can roll back roster files"
type = "bug"
category = "in-progress"
priority = "low"
points = 2
reporter = "lognd"
created = "2026-10-17T21:41:51Z"
updated = "2026-10-17T22:07:43Z"
idempotency_key = "audit-l1l9"
scope = ["crates/goway/src/circle.rs", "crates/goway/src/changelog.rs", "crates/goway/tests/circle.rs", "crates/goway/src/circle_admin.rs"]

[[acceptance]]
text = "Unapplied ops are never stored as heads; undo refuses circle store paths"
bound = true
+++

See audit-0.2.0.md L1 and L9. Keep only applied ops before commit and derive parents from applied heads; mark circle store writes not reversible or make undo refuse paths under circle/circles/.
