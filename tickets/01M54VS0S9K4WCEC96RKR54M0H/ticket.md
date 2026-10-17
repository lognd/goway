+++
id = "01M54VS0S9K4WCEC96RKR54M0H"
title = "Wire the passphrase prompt into goway circle commands (init, join, admin key use)"
type = "bug"
category = "done"
outcome = "done"
priority = "medium"
points = 2
parent = "01M54433AG8J1V0GYT3ZWY9C9A"
reporter = "lognd"
created = "2026-10-17T12:00:53Z"
updated = "2026-10-17T12:22:57Z"
scope = ["crates/goway/src/circle.rs", "crates/goway/src/cli.rs", "crates/goway/src/access.rs", "crates/goway/src/lanpair.rs", "crates/goway/tests/circle_*.rs", "docs/circles.md"]

[[acceptance]]
text = "An encrypted circle key signs only with its passphrase, and a plain key never prompts"
bound = true

[[acceptance]]
text = "goway circle init --passphrase creates encrypted keys"
bound = true
+++
