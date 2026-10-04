+++
id = "01M426GBSHQE6DMEBRJAD8JP8D"
title = "Windows elevation: detect a desktop session by session id (not SESSIONNAME), and --rsudo/--lsudo elevate Windows-side steps through an admin OpenSSH session or UAC, never with a stored password"
type = "story"
category = "done"
outcome = "done"
priority = "high"
points = 5
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-04T00:54:59Z"
updated = "2026-10-04T02:15:47Z"
scope = ["crates/goway-setup/src/elevate.rs", "crates/goway-setup/src/cli.rs", "crates/goway-setup/tests/elevate.rs", "docs/install-windows.md", "crates/goway-setup/Cargo.toml"]

[[acceptance]]
text = "Given goway-setup started from WSL interop or any process without SESSIONNAME but running in the active console session, when it needs elevation, then it detects the interactive desktop from its own session id (ProcessIdToSessionId against WTSGetActiveConsoleSessionId) and shows the UAC prompt; only a process outside the active desktop session (session 0, a remote non-admin session) is told to use an elevated terminal (found live on 2026-10-03: install --host through ssh into WSL was refused although session 1 could prompt)"
bound = true
+++
