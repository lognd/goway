+++
id = "01M41BWCNK0QQQC119DNPS2P1Y"
title = "New worktree seeds start from a sibling seed of the same repository"
type = "task"
category = "in-progress"
priority = "medium"
points = 2
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-03T17:09:41Z"
updated = "2026-10-03T17:21:34Z"
scope = ["crates/goway/**", "docs/usage.md"]

[[acceptance]]
text = "Given a repository already synced from one worktree, when a second worktree runs for the first time, then only files that differ between the worktrees are sent"
bound = true

[[acceptance]]
text = "Given a sync that deletes more files than fit in one command-line argument, when goway syncs, then the deletion succeeds"
bound = true

[[acceptance]]
text = "Given gc removes a seed between the manifest and the upload, when goway syncs, then it detects the changed seed generation and resyncs fully instead of running on a partial tree"
bound = false
+++
