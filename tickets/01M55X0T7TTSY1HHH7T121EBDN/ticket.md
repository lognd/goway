+++
id = "01M55X0T7TTSY1HHH7T121EBDN"
title = "Audit L5+L8: anti-rollback docs and circle pin collision with manual host"
type = "bug"
category = "todo"
priority = "low"
points = 2
reporter = "lognd"
created = "2026-10-17T21:41:51Z"
updated = "2026-10-17T21:41:51Z"
idempotency_key = "audit-l5l8"
scope = ["crates/goway/src/access.rs"]

[[acceptance]]
text = "pin refuses name collision; docs accurate"
bound = false
+++

See audit-0.2.0.md L5 (docs/circles.md) and L8. Correct docs to match single global head set, no --accept-reset; refuse circle pin when a manual [[host]] has that name.
