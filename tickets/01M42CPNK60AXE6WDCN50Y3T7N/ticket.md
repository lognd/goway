+++
id = "01M42CPNK60AXE6WDCN50Y3T7N"
title = "goway add asks whether the user knows the helper password before trying one, and ssh gets one password attempt so a wrong or blank one falls through to the key path at once"
type = "story"
category = "in-progress"
priority = "high"
points = 2
parent = "01M42BHPTWVNZ4DZ0H4GXG60MX"
reporter = "lognd"
created = "2026-10-04T02:43:17Z"
updated = "2026-10-04T03:00:17Z"
scope = ["crates/goway/src/add.rs", "crates/goway/src/sshsetup.rs", "crates/goway/src/ssh.rs", "crates/goway/tests/ssh_setup.rs", "docs/troubleshooting.md", "crates/goway/src/remotesys.rs", "docs/ssh-setup.md"]

[[acceptance]]
text = "Given goway add on a terminal for a helper without goway's key, when the key must be installed, then goway first asks 'Do you know the password of USER on HOST? [Y/n]' (no flag needed); n goes straight to the by-hand key path; --no-password and --yes keep working for scripts"
bound = true

[[acceptance]]
text = "Given the password login, when ssh asks, then it asks exactly once (NumberOfPasswordPrompts=1), so a wrong or empty password (an account with no password cannot log in over ssh at all) falls through to the by-hand key path immediately with the cause list, and never makes more than one failed attempt toward a fail2ban limit"
bound = false
+++
