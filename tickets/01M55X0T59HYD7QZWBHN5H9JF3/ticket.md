+++
id = "01M55X0T59HYD7QZWBHN5H9JF3"
title = "Audit L4: mint lead certs with documented forwarding bans"
type = "bug"
category = "in-progress"
priority = "low"
points = 2
reporter = "lognd"
created = "2026-10-17T21:41:51Z"
updated = "2026-10-17T21:45:36Z"
idempotency_key = "audit-l4l7"
scope = ["crates/goway-circle/src/sshcert.rs"]

[[acceptance]]
text = "Minted certs carry the no-forwarding options"
bound = false
+++

See audit-0.2.0.md L4 and L7. Mint certs with documented -O options; put short circle id in marker, still match old form when removing.
