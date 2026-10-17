+++
id = "01M54BPQQM89SWXDHMJX6C25C2"
title = "Fold docs/circle-admin.md into circles.md and config.md; refresh 0.2.0 wording and roster stage"
type = "docs"
category = "in-progress"
priority = "medium"
points = 1
reporter = "lognd"
created = "2026-10-17T07:20:01Z"
updated = "2026-10-17T07:20:26Z"
idempotency_key = "docs-circles-fold"
scope = ["docs/circles.md", "docs/config.md", "docs/circle-admin.md"]

[[acceptance]]
text = "docs/circle-admin.md is folded into docs/circles.md and docs/config.md and removed; circles.md status and roster-stage text match the code"
bound = false
+++

Merge docs/circle-admin.md (admin add|remove|cosign|card, renew, policy) into docs/circles.md and docs/config.md then delete it; fix stale 'target release 0.2.0' and 'phases 1 to 3' wording; replace 'Not yet: roster stage' in 4.3 and add a roster-stage paragraph in 4.5.
