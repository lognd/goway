+++
id = "01M5493X89KGCWGBCRB7JMKV3B"
title = "Built-in passphrase prompt and encrypted node key (ssh-keygen prompt works today)"
type = "task"
category = "in-progress"
priority = "medium"
points = 3
reporter = "lognd"
created = "2026-10-17T06:34:47Z"
updated = "2026-10-17T12:00:49Z"
scope = ["crates/goway-circle/**", "docs/circles.md"]

[[acceptance]]
text = "A node key can be created encrypted and signing works only with the right passphrase"
bound = true

[[acceptance]]
text = "Passphrase values are redacted in Debug and empty passphrases are refused"
bound = true
+++

found while working ~MV967CF; deferred hardening, see docs/circles.md section 6
