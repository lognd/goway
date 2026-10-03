+++
id = "01M41ZE1AJB704T3YPAK2BXN3N"
title = "CI: copy_integrity tests are Unix-only; the shares note lists hosts in a random order"
type = "bug"
category = "todo"
priority = "high"
points = 1
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-03T22:51:22Z"
updated = "2026-10-03T22:51:22Z"
scope = ["crates/goway/tests/copy_integrity.rs", "crates/goway/src/shard.rs", "crates/goway/src/runners.rs", "crates/goway/tests/weighted_shards.rs"]

[[acceptance]]
text = "Given Windows CI, when it builds the tests, then copy_integrity.rs compiles (it runs only on Unix like the other remote-script tests)"
bound = false

[[acceptance]]
text = "Given a weighted sharded run, when goway prints the shares note, then hosts are listed by share, largest first, then by name, so output and tests are deterministic"
bound = false
+++
