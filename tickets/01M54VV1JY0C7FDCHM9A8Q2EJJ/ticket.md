+++
id = "01M54VV1JY0C7FDCHM9A8Q2EJJ"
title = "Add a goway circle command that calls generate_sk_keypair for admin keys"
type = "bug"
category = "in-progress"
priority = "medium"
points = 2
parent = "01M54433AG8J1V0GYT3ZWY9C9A"
reporter = "lognd"
created = "2026-10-17T12:01:59Z"
updated = "2026-10-17T12:06:34Z"
scope = ["crates/goway/src/circle_admin.rs", "crates/goway/src/cli.rs", "crates/goway/tests/circle_admin.rs", "docs/circles.md"]

[[acceptance]]
text = "goway circle admin keygen runs generate_sk_keypair for this machine's admin key and refuses to overwrite an existing key"
bound = false
+++
