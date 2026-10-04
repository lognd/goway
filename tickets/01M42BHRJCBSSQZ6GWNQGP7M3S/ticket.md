+++
id = "01M42BHRJCBSSQZ6GWNQGP7M3S"
title = "Shell startup files that print text or a non-bash login shell must not corrupt goway's protocol"
type = "bug"
category = "in-progress"
priority = "high"
points = 3
parent = "01M42BHPTWVNZ4DZ0H4GXG60MX"
reporter = "lognd"
created = "2026-10-04T02:23:07Z"
updated = "2026-10-04T03:08:10Z"
scope = ["crates/goway/src/ssh.rs", "crates/goway/src/remote.rs", "crates/goway/src/remote.sh", "crates/goway/tests/shell_noise.rs", "docs/troubleshooting.md", "crates/goway/src/sync.rs", "crates/goway/src/resolve.rs", "crates/goway/src/transport.rs", "crates/goway/src/run.rs", "crates/goway/src/local.rs", "crates/goway/src/gc.rs", "crates/goway/tests/host_facts.rs"]

[[acceptance]]
text = "Given a helper whose .bashrc prints text for non-interactive ssh commands, or whose login shell is fish or csh, when goway syncs, runs and probes, then the protocol output stays exact (for example a framed marker before the payload, and the command passed so that any login shell executes bash with it unchanged), and doctor warns about noisy startup files with the line to move"
bound = false
+++
