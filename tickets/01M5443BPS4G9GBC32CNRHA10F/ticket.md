+++
id = "01M5443BPS4G9GBC32CNRHA10F"
title = "Phase 2: goway circle card/admit/join/list/revoke and goway-setup --join"
type = "story"
category = "done"
outcome = "done"
priority = "high"
points = 8
parent = "01M54433AG8J1V0GYT3ZWY9C9A"
reporter = "lognd"
created = "2026-10-17T05:07:06Z"
updated = "2026-10-17T05:57:23Z"
idempotency_key = "c2"
labels = ["circles"]
scope = ["Cargo.toml", "Cargo.lock", "crates/goway-circle/**", "crates/goway/Cargo.toml", "crates/goway/tests/journal_invariant.rs", "crates/goway/src/circle.rs", "crates/goway/src/error.rs", "crates/goway/src/lib.rs", "crates/goway/src/changelog.rs", "crates/goway/src/sshsetup.rs", "docs/circles.md", "docs/config.md", "docs/glossary.md", "changelog.d/01M5443BPS4G9GBC32CNRHA10F.added.md", "crates/goway/src/cli.rs"]

[[acceptance]]
text = "A helper and a lead join with no network pairing"
bound = true

[[acceptance]]
text = "Cards and welcomes never logged"
bound = true

[[acceptance]]
text = "A wrong fingerprint on admit or join changes nothing"
bound = true

[[acceptance]]
text = "Revoke and roles change the roster; a revoked key cannot be re-admitted"
bound = true

[[acceptance]]
text = "Node and admin private keys are created readable only by their owner and every roster write is journaled"
bound = true
+++

Offline two-paste pairing (gwc1 card, gww1 welcome), fingerprint confirm, journaled files, logging, docs/ updates.
