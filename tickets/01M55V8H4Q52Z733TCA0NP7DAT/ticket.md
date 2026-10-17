+++
id = "01M55V8H4Q52Z733TCA0NP7DAT"
title = "Audit M8: access --windows-admin rewrites administrators_authorized_keys without its protected DACL"
type = "task"
category = "in-progress"
priority = "medium"
points = 3
parent = "01M54433AG8J1V0GYT3ZWY9C9A"
reporter = "lognd"
created = "2026-10-17T21:11:07Z"
updated = "2026-10-17T21:22:13Z"
idempotency_key = "audit-m8"
scope = ["crates/goway/src/access.rs", "crates/goway-journal/src/model.rs", "crates/goway-journal/src/lib.rs", "crates/goway-setup/src/native.rs", "changelog.d/01M55V8H4Q52Z733TCA0NP7DAT.fixed.md"]

[[acceptance]]
text = "Given access --windows-admin, when the file is rewritten, then the protected ADMIN_KEYS_SDDL is applied after writing"
bound = true
+++

Audit M8: apply ADMIN_KEYS_SDDL after writing; unverified on real Windows, so cover with a unit test of the command plan.
