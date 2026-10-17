+++
id = "01M5493X9E88JC1ECXBSKGF7C1"
title = "Automated generation of ed25519-sk admin keys (manual ssh-keygen today)"
type = "task"
category = "in-progress"
priority = "medium"
points = 3
reporter = "lognd"
created = "2026-10-17T06:34:47Z"
updated = "2026-10-17T12:01:37Z"
scope = ["crates/goway-circle/**"]

[[acceptance]]
text = "generate_sk_keypair runs ssh-keygen -t ed25519-sk through an injectable program and returns the key pair"
bound = false

[[acceptance]]
text = "generate_sk_keypair reports tool failure and never overwrites an existing key"
bound = false
+++

found while working ~MV967CF; deferred hardening, see docs/circles.md section 6
