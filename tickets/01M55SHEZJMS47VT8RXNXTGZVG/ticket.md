+++
id = "01M55SHEZJMS47VT8RXNXTGZVG"
title = "Audit M6: OS keystore saves a typed passphrase before it is verified"
type = "security"
category = "done"
outcome = "done"
priority = "medium"
points = 3
parent = "01M54433AG8J1V0GYT3ZWY9C9A"
reporter = "lognd"
created = "2026-10-17T20:41:03Z"
updated = "2026-10-17T20:45:31Z"
idempotency_key = "audit-m6"
scope = ["crates/goway-circle/src/keystore.rs,crates/goway-circle/src/passphrase.rs", "crates/goway-circle/src/lib.rs", "changelog.d/01M55SHEZJMS47VT8RXNXTGZVG.fixed.md", "crates/goway-circle/src/keystore.rs", "crates/goway-circle/src/passphrase.rs"]

[[acceptance]]
text = "Given a mistyped passphrase, when it is entered, then it is not stored until verified"
bound = true
+++

Verify against the key before saving.
