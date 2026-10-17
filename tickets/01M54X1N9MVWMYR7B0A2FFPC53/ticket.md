+++
id = "01M54X1N9MVWMYR7B0A2FFPC53"
title = "circle init --passphrase asks once with no confirmation and has no test of the encrypted-key path"
type = "bug"
category = "in-progress"
priority = "medium"
points = 2
parent = "01M54433AG8J1V0GYT3ZWY9C9A"
reporter = "lognd"
created = "2026-10-17T12:23:05Z"
updated = "2026-10-17T12:42:33Z"
scope = ["crates/goway/src/circle.rs", "crates/goway-circle/src/**", "docs/circles.md"]

[[acceptance]]
text = "init --passphrase asks twice and refuses a mismatched confirmation, making no keys"
bound = true

[[acceptance]]
text = "a matching pair yields encrypted keys that sign only with the passphrase"
bound = true

[[acceptance]]
text = "the double-entry check accepts equal entries and rejects differing ones"
bound = true
+++
