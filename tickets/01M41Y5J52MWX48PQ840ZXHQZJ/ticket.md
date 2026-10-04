+++
id = "01M41Y5J52MWX48PQ840ZXHQZJ"
title = "Disk budget per helper: cap goway's total disk use and evict least-recently-used caches, slots and dependency dirs"
type = "story"
category = "in-progress"
priority = "high"
points = 3
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-03T22:29:16Z"
updated = "2026-10-04T00:01:51Z"
scope = ["crates/goway/src/remote.sh", "crates/goway/src/gc.rs", "crates/goway/src/config.rs", "crates/goway/src/status.rs", "crates/goway/src/run.rs", "docs/config.md", "docs/usage.md", "crates/goway/tests/disk_budget.rs", "crates/goway/src/pool.rs", "crates/goway/src/needs.rs"]

[[acceptance]]
text = "Given max_disk (default the smaller of 20% of the helper's disk and 50 GiB) and min_free (default 10 GiB), when a run or auto-gc finds goway's root over budget or the disk below min_free, then it evicts unlocked entries least recently used first (slot trees with their target and build dirs, seeds of idle worktrees, per-repository caches) until within budget, never touching an entry in use, and says what it freed"
bound = true

[[acceptance]]
text = "Given sccache or ccache caches per repository, when goway starts them, then their size limit is set (SCCACHE_CACHE_SIZE, CCACHE_MAXSIZE; default 2 GiB per repository, configurable) unless the user set it"
bound = true

[[acceptance]]
text = "Given goway status, when it runs, then each host shows goway's disk use against its budget, and goway gc --dry-run lists what eviction would remove"
bound = false
+++
