+++
id = "01M41BWCNK0QQQC119DNPS2P1Y"
title = "New worktree seeds start from a sibling seed of the same repository"
type = "task"
category = "todo"
priority = "medium"
points = 2
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-03T17:09:41Z"
updated = "2026-10-03T17:10:30Z"
scope = ["crates/goway/**", "docs/usage.md"]

[[acceptance]]
text = "Given a repository already synced from one worktree, when a second worktree runs for the first time, then only files that differ between the worktrees are sent"
bound = false

[[acceptance]]
text = "Given a sync that deletes more files than fit in one command-line argument, when goway syncs, then the deletion succeeds"
bound = false
+++
