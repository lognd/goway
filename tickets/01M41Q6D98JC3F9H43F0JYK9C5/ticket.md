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
updated = "2026-10-03T20:29:24Z"
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

[[acceptance]]
text = "Given an NTFS copy, a cargo target dir or a run folder on a Windows host, when it is past its expiry (7 days for caches, 1 day for orphaned run folders, same config as other hosts) or its repository was removed, then the automatic gc on the next run deletes it, and goway gc --older-than/--repo/--all removes it on demand"
bound = false
+++
