+++
id = "01M55V8H0SBF8BY25NQP023524"
title = "Audit M4: revoked lead in a saved listed policy breaks circle access"
type = "task"
category = "todo"
priority = "medium"
points = 3
parent = "01M54433AG8J1V0GYT3ZWY9C9A"
reporter = "lognd"
created = "2026-10-17T21:11:07Z"
updated = "2026-10-17T21:11:07Z"
idempotency_key = "audit-m4"
scope = ["crates/goway/src/access.rs"]

[[acceptance]]
text = "Given a saved listed policy naming a revoked member, when access runs, then it drops and warns about the name and writes the new-epoch lines"
bound = false
+++

Audit M4 in /tmp scratchpad audit-0.2.0.md: in run_access, drop names that are no longer members from the SAVED policy with a warning; keep the hard error only for explicit --lead.
