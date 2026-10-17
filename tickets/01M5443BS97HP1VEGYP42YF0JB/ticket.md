+++
id = "01M5443BS97HP1VEGYP42YF0JB"
title = "Phase 4: LAN pairing (CPace + SAS), QR, peer address hints"
type = "story"
category = "in-progress"
priority = "medium"
points = 8
parent = "01M54433AG8J1V0GYT3ZWY9C9A"
reporter = "lognd"
created = "2026-10-17T05:07:06Z"
updated = "2026-10-17T06:23:11Z"
idempotency_key = "c4"
labels = ["circles"]
scope = ["crates/goway-circle/Cargo.toml", "crates/goway-circle/src/lan.rs", "crates/goway-circle/src/lib.rs", "crates/goway/src/circle.rs", "crates/goway/src/cli.rs", "crates/goway/src/lanpair.rs", "crates/goway/src/lib.rs", "crates/goway/src/resolve.rs", "crates/goway/src/state.rs", "docs/circles.md", "docs/hosts.md", "changelog.d/01M5443BS97HP1VEGYP42YF0JB.added.md", "Cargo.lock"]
+++

Short-lived single-use rate-limited listener; no circle identity in mDNS; unsigned address hints as a resolve stage.
