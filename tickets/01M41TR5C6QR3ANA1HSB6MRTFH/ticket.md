+++
id = "01M41TR5C6QR3ANA1HSB6MRTFH"
title = "Copy integrity: verify changed files every run, verify everything on failure, rerun once only on a proven bad copy, and distrust the fast path afterwards"
type = "story"
category = "todo"
priority = "high"
points = 5
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-03T21:29:31Z"
updated = "2026-10-03T21:29:32Z"
scope = ["crates/goway/src/remote.sh", "crates/goway/src/run.rs", "crates/goway/src/sync.rs", "crates/goway/src/state.rs", "crates/goway/src/shard.rs", "crates/goway/src/cli.rs", "crates/goway/src/gc.rs", "crates/goway/tests/**", "docs/design.md", "docs/usage.md", "SECURITY.md"]

[[links]]
kind = "blocked-by"
target = "01M41TNZH9N2J377DRPQNTX8FZ"

[[acceptance]]
text = "Given any run, when the helper has reconciled its slot tree and before the command starts, then it reports bounded, strictly parsed SHA-256 hashes of every path this sync changed and goway compares them with the laptop's; a mismatch stops the command from starting, rebuilds the slot from scratch and says so"
bound = false

[[acceptance]]
text = "Given a run that exits non-zero, when it ends, then goway compares hashes of every synced file in the run's tree with the work tree; if all match, the failure is reported with the command's exit code and never rerun; if any differ, goway names the files, rebuilds the slot from scratch, reruns the command exactly once, and the report and --report JSON record both attempts with the first marked invalid"
bound = false

[[acceptance]]
text = "Given a proven mismatch for a repository on a host, when later runs start, then that repository on that host gets full verification and a fresh slot copy every run until 7 days pass without a mismatch, goway gc --repo clears it, or --trust-copy overrides one run; the marker lives only in goway's local state, never in the repository or goway.toml"
bound = false

[[acceptance]]
text = "Given a mismatch, when it is reported, then goway says it is a goway bug and prints what to include in a report (host, repository id, paths and sizes, never file contents)"
bound = false

[[acceptance]]
text = "Given SECURITY.md, when read, then it states that these checks guard against goway's own bugs and not against a compromised helper, which can already report anything"
bound = false
+++
