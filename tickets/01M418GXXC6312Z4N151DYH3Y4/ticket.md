+++
id = "01M418GXXC6312Z4N151DYH3Y4"
title = "goway v0.1: offload commands to a pool of WSL hosts"
type = "epic"
category = "todo"
priority = "medium"
points = 13
reporter = "lognd"
created = "2026-10-03T16:11:00Z"
updated = "2026-10-03T16:11:00Z"
scope = ["crates/**", "docs/**", "scripts/**"]

[[acceptance]]
text = "Given two configured hosts, when goway run -- cargo nextest run is invoked from a worktree, then it runs on the least-loaded host and exits with the command's exit code"
bound = false
+++

See docs/design.md.
