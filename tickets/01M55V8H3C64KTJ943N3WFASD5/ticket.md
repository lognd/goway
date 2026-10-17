+++
id = "01M55V8H3C64KTJ943N3WFASD5"
title = "Audit M9: goway-setup --join passes the whole welcome on the command line"
type = "task"
category = "in-progress"
priority = "medium"
points = 5
parent = "01M54433AG8J1V0GYT3ZWY9C9A"
reporter = "lognd"
created = "2026-10-17T21:11:07Z"
updated = "2026-10-17T21:11:50Z"
idempotency_key = "audit-m9"
scope = ["crates/goway-setup/src/hostsys.rs", "crates/goway/src/circle.rs", "crates/goway-setup/tests/hostsys.rs"]

[[acceptance]]
text = "Given a welcome of 40 ops, when goway-setup joins, then the welcome travels on stdin and the command line stays short"
bound = false
+++

Audit M9: pass the welcome on stdin (goway circle join -, mirroring sync -) instead of argv; Windows command line limit hit at about 25 ops.
