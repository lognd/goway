+++
id = "01M42E8Z94EFYWZ4XYMMR97WNB"
title = "Tests use git init -b (git 2.28+); the suite must run on helpers with older git (Ubuntu 20.04 has git 2.25)"
type = "bug"
category = "in-progress"
priority = "medium"
points = 1
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-04T03:10:45Z"
updated = "2026-10-04T04:05:17Z"
scope = ["crates/goway/tests/common/mod.rs", "crates/goway/src/repo.rs", "crates/goway/src/sync.rs", "crates/goway/src/run.rs", "docs/hosts.md"]

[[acceptance]]
text = "Given a helper with git 2.25 (Ubuntu 20.04; found live on a real helper: sync::tests::a_non_utf8_file_name_fails_loudly failed with 'unknown switch b'), when the goway suite runs there, then every test that creates a repository uses one shared helper that works on old git (git init -q then git symbolic-ref HEAD refs/heads/main), and docs/hosts.md states the oldest git goway itself needs (the product needs no git on helpers; the laptop needs git for ls-files -co --exclude-standard -z)"
bound = true
+++
