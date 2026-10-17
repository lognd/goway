+++
id = "01M55V8H24YCCZ92VT244937EV"
title = "Audit M5: cert-authority lines never expire; docs describe nonexistent verbs"
type = "task"
category = "in-progress"
priority = "medium"
points = 3
parent = "01M54433AG8J1V0GYT3ZWY9C9A"
reporter = "lognd"
created = "2026-10-17T21:11:07Z"
updated = "2026-10-17T21:17:21Z"
idempotency_key = "audit-m5"
scope = ["crates/goway-circle/src/sshcert.rs", "docs/circles.md", "changelog.d/01M55V8H24YCCZ92VT244937EV.changed.md"]

[[acceptance]]
text = "Given authority_lines, when written, then each line carries an expiry-time of now plus twice the TTL and docs no longer claim circle-heads or circle-exchange exist"
bound = true
+++

Audit M5: add expiry-time to every cert-authority line so an unrefreshed helper fails closed; fix docs/circles.md passages near lines 138 and 281.
