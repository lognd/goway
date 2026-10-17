+++
id = "01M5443BQZFS1KV0S4JQ5WP11P"
title = "Phase 3: certificates, authorized_keys cert-authority lines, roster-fed pinning"
type = "story"
category = "in-progress"
priority = "high"
points = 8
parent = "01M54433AG8J1V0GYT3ZWY9C9A"
reporter = "lognd"
created = "2026-10-17T05:07:06Z"
updated = "2026-10-17T06:25:00Z"
idempotency_key = "c3"
labels = ["circles"]
scope = ["crates/goway/tests/journal_invariant.rs", "crates/goway-circle/src/error.rs", "crates/goway-circle/src/lib.rs", "crates/goway-circle/src/sshcert.rs"]

[[acceptance]]
text = "New lead runs on every helper with no helper edit"
bound = true

[[acceptance]]
text = "Revoke bumps epoch and old certs fail"
bound = true
+++

Cert minting and renewal, epoch principals, helper policy open/listed, resolve.rs stage for roster hosts, config merge with manual hosts, Windows administrators_authorized_keys, doctor checks for ssh-keygen -Y and clock skew.
