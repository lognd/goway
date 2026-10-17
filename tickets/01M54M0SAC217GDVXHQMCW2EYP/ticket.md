+++
id = "01M54M0SAC217GDVXHQMCW2EYP"
title = "goway-setup: journal the distro goway symlink and run native join as the sshd account"
type = "task"
category = "in-progress"
priority = "medium"
points = 3
parent = "01M54433AG8J1V0GYT3ZWY9C9A"
reporter = "lognd"
created = "2026-10-17T09:45:19Z"
updated = "2026-10-17T11:13:07Z"
scope = ["crates/goway-setup/**", "docs/install-windows.md", "changelog.d/01M54M0SAC217GDVXHQMCW2EYP.fixed.md"]

[[acceptance]]
text = "The distro goway wrapper is a journaled change, written only when goway is not on PATH"
bound = true

[[acceptance]]
text = "Native join is not run as an account other than the ssh login account"
bound = true
+++

found while working ~SG8MFPH: /usr/local/bin/goway link is not journaled (undo does not remove it); native join runs as the invoking user, not the sshd account if different
