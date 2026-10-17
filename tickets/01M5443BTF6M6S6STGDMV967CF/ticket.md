+++
id = "01M5443BTF6M6S6STGDMV967CF"
title = "Security: circle threat model review and key-at-rest hardening"
type = "security"
category = "in-progress"
priority = "high"
points = 8
parent = "01M54433AG8J1V0GYT3ZWY9C9A"
reporter = "lognd"
created = "2026-10-17T05:07:06Z"
updated = "2026-10-17T06:28:06Z"
idempotency_key = "c5"
labels = ["circles"]
scope = ["crates/goway-circle/src/keys.rs", "crates/goway-circle/src/state.rs", "crates/goway-circle/src/store.rs", "crates/goway-circle/src/sshcert.rs", "crates/goway-circle/src/crypto.rs", "crates/goway-circle/src/error.rs"]
+++

Walk the 16-row table in docs/circles.md against the code; agent/keystore support for the node key (config.md notes the unencrypted key); FIDO2 admin key path; add frob rules for the invariants.
