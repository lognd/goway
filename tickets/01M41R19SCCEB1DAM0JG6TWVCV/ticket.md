+++
id = "01M41R19SCCEB1DAM0JG6TWVCV"
title = "gc never removes a work dir that was created moments ago"
type = "security"
category = "in-progress"
priority = "medium"
points = 1
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-03T20:42:05Z"
updated = "2026-10-03T20:43:39Z"
scope = ["crates/goway/src/remote.sh", "crates/goway/tests/remote_root.rs", "docs/usage.md"]

[[acceptance]]
text = "Given a fresh work dir with no lock yet, When gc --all runs, Then the dir is kept"
bound = true
+++
