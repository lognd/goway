+++
id = "01M546WXG3D6ADTBCZH4EW94KV"
title = "goway circle revoke --now (epoch push), certificate expiry and last seen in list"
type = "task"
category = "in-progress"
priority = "medium"
points = 3
parent = "01M54433AG8J1V0GYT3ZWY9C9A"
reporter = "lognd"
created = "2026-10-17T05:56:01Z"
updated = "2026-10-17T07:25:11Z"
labels = ["circles"]
scope = ["crates/goway/src/circle_push.rs", "crates/goway/src/circle.rs", "crates/goway/src/access.rs", "crates/goway/src/cli.rs", "crates/goway/src/lib.rs", "docs/circles.md", "crates/goway/tests/circle_revoke_now.rs", "changelog.d/01M4EW94KV*.md", "changelog.d/*4EW94KV*", "changelog.d/01M54BQ0000000000000004EW94KV.added.md", "changelog.d/01M546WXG3D6ADTBCZH4EW94KV.added.md"]

[[acceptance]]
text = "goway circle revoke --now asks every other roster helper to refresh its access lines, records who answered, names unreachable helpers, and goway circle list shows cert expiry and last seen"
bound = true
+++

deferred from ~NRHA10F (circles phase 2)
