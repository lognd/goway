+++
id = "01M55X0T3ZVTASAQTZDDZ9JCA3"
title = "Audit L2+L3: LAN pairing single-use, pre-PAKE timeout, welcome tag"
type = "bug"
category = "in-progress"
priority = "low"
points = 2
reporter = "lognd"
created = "2026-10-17T21:41:51Z"
updated = "2026-10-17T21:41:59Z"
idempotency_key = "audit-l2l3"
scope = ["crates/goway/src/lanpair.rs", "crates/goway-circle/src/lan.rs"]

[[acceptance]]
text = "Listener closes after first comparison; welcome authenticated"
bound = false
+++

See audit-0.2.0.md L2 and L3. consume() on guest card tag check; shorten pre-PAKE timeout; add tag(isk,welcome) verified by guest in goway-circle lan.rs.
