+++
id = "01M55X0T59HYD7QZWBHN5H9JF3"
title = "Audit L4+L7: cert options and circle-name marker collision"
type = "bug"
category = "todo"
priority = "low"
points = 2
reporter = "lognd"
created = "2026-10-17T21:41:51Z"
updated = "2026-10-17T21:41:51Z"
idempotency_key = "audit-l4l7"
scope = ["crates/goway-circle/src/sshcert.rs"]

[[acceptance]]
text = "Certs carry no-forwarding options; same-name circles do not overwrite"
bound = false
+++

See audit-0.2.0.md L4 and L7. Mint certs with documented -O options; put short circle id in marker, still match old form when removing.
