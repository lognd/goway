+++
id = "01M55SHERW6M3D3ZXM6Y1EKY38"
title = "Audit H1: a revoked admin key can act by backdating its parents"
type = "security"
category = "todo"
priority = "critical"
points = 3
parent = "01M54433AG8J1V0GYT3ZWY9C9A"
reporter = "lognd"
created = "2026-10-17T20:41:02Z"
updated = "2026-10-17T20:41:02Z"
idempotency_key = "audit-h1"
scope = ["crates/goway-circle/src/state.rs,crates/goway-circle/tests/**,docs/model/**"]

[[acceptance]]
text = "Given a stolen revoked admin key choosing old parents, when the roster is replayed, then its operations after the removal are ignored"
bound = false
+++

Replay judges each op against the state implied by its self-chosen parents. Check against the op's own history and ignore ops by a removed key not before its removal. See scratchpad audit-0.2.0.md H1.
