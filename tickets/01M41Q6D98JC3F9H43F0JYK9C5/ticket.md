+++
id = "01M41Q6D98JC3F9H43F0JYK9C5"
title = "Sync a WSL work tree to an NTFS copy per repository and worktree, incrementally, without scanning across drvfs"
type = "story"
category = "todo"
priority = "medium"
points = 5
parent = "01M41Q6CAC5EW88V3ZFSFJ1T6J"
reporter = "lognd"
created = "2026-10-03T20:27:24Z"
updated = "2026-10-03T20:27:24Z"
scope = ["crates/goway/src/sync.rs", "crates/goway/src/interop.rs", "crates/goway/tests/**", "docs/design.md"]

[[acceptance]]
text = "Given an interop host, when goway syncs, then the file set is git ls-files -co --exclude-standard minus secrets, written into goway's own NTFS directory for that repository and worktree, never another repository's"
bound = false

[[acceptance]]
text = "Given a repeat sync, when few files changed, then only those are written, decided on the WSL side without stat-scanning the NTFS copy, and deletions touch only goway's own directory"
bound = false

[[acceptance]]
text = "Given Windows paths, when goway translates them, then it uses wslpath and refuses paths outside goway's root"
bound = false
+++
