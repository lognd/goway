+++
id = "01M41SXDVCQ4M6ZXSAVG0F0TN1"
title = "Windows CI: slot_trees tests use Unix-only APIs"
type = "bug"
category = "done"
outcome = "wont-fix"
priority = "medium"
points = 1
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-03T21:14:55Z"
updated = "2026-10-03T21:15:04Z"
scope = ["crates/goway/tests/slot_trees.rs"]

[[acceptance]]
text = "Given Windows CI, when clippy and the tests build, then slot_trees.rs compiles (it runs only on Unix, like the other remote-script tests)"
bound = false
+++
