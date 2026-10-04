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
updated = "2026-10-04T00:22:01Z"
scope = ["crates/goway/src/sync.rs", "crates/goway/src/interop.rs", "docs/design.md", "crates/goway/src/gc.rs", "crates/goway/src/run.rs", "crates/goway/src/pool.rs", "crates/goway/src/status.rs", "crates/goway/src/transport.rs", "crates/goway/src/remote.ps1"]

[[links]]
kind = "blocked-by"
target = "01M41P2FZ22KC8PJVR9RY6QS01"

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

[[acceptance]]
text = "Given repeated runs on a Windows host, when they reuse a slot, then the slot tree is updated in place like on Linux (persistent slot trees), not copied and deleted per run"
bound = false

[[acceptance]]
text = "Given a [[host]] with os = windows (interop or ssh), when goway run, status, gc or a sharded run picks it, then the host kind is wired through the whole run, sync and pool pipeline (sync.rs Transport, run.rs), not only config and host add"
bound = false
+++
