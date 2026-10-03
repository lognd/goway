+++
id = "01M41SDVD25MA7AX1DHNMXT7K6"
title = "Local host: --host local runs here in place; [local] pool opt-in; offline failure offers local"
type = "story"
category = "in-progress"
priority = "medium"
points = 5
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-03T21:06:25Z"
updated = "2026-10-03T22:12:25Z"
scope = ["crates/goway/src/pool.rs", "crates/goway/src/run.rs", "crates/goway/src/config.rs", "crates/goway/src/local.rs", "crates/goway/src/shard.rs", "crates/goway/src/status.rs", "crates/goway/src/error.rs", "crates/goway/tests/**", "docs/usage.md", "docs/config.md", "crates/goway/src/resolve.rs", "crates/goway/src/lib.rs"]

[[acceptance]]
text = "Given goway run --host local -- CMD, when it runs, then CMD runs in the current work tree with no sync, at the configured priority, with live output, the command's exit code, and host local plus this machine's architecture in the report"
bound = true

[[acceptance]]
text = "Given no [local] section or pool = false, when goway picks hosts or shards, then local is never chosen; given [local] pool = true (with max_jobs, default 1, and priority), then local competes on load with a margin for interactive use and can take shards"
bound = false

[[acceptance]]
text = "Given no helper is reachable, when goway run picks a host, then it exits 125 with the reason and a next step naming --host local and [local] fallback = true; with fallback = true it runs locally with a clear note and the report records host local"
bound = false

[[acceptance]]
text = "Given goway status, when local is configured, then it shows a local row with its load and whether it is in the pool"
bound = false
+++
