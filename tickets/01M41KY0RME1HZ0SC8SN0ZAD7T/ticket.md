+++
id = "01M41KY0RME1HZ0SC8SN0ZAD7T"
title = "CI: unix-only test helpers unused on Windows; purge races the post-run gc"
type = "bug"
category = "in-progress"
priority = "medium"
points = 1
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-03T19:30:23Z"
updated = "2026-10-03T19:58:56Z"
scope = ["crates/goway/src/**", "crates/goway-journal/tests/**", "crates/goway-setup/tests/**", ".github/workflows/ci.yml", "frob.toml"]

[[acceptance]]
text = "Given the CI workflow, when it runs on Linux and Windows, then both jobs pass"
bound = true

[[acceptance]]
text = "Given goway uninstall right after a run, when the post-run gc still holds a lock briefly, then purge waits for it instead of reporting a run in progress"
bound = true
+++
