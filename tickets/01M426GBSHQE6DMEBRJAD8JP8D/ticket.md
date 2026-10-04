+++
id = "01M426GBSHQE6DMEBRJAD8JP8D"
title = "Windows elevation: detect a desktop session by session id (not SESSIONNAME), and --rsudo/--lsudo elevate Windows-side steps through an admin OpenSSH session or UAC, never with a stored password"
type = "story"
category = "todo"
priority = "high"
points = 5
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-04T00:54:59Z"
updated = "2026-10-04T00:54:59Z"
scope = ["crates/goway-setup/src/elevate.rs", "crates/goway-setup/src/cli.rs", "crates/goway-setup/tests/elevate.rs", "crates/goway/src/add.rs", "crates/goway/src/doctor.rs", "crates/goway/src/cli.rs", "docs/install-windows.md", "docs/usage.md"]

[[acceptance]]
text = "Given goway-setup started from WSL interop or any process without SESSIONNAME but running in the active console session, when it needs elevation, then it detects the interactive desktop from its own session id (ProcessIdToSessionId against WTSGetActiveConsoleSessionId) and shows the UAC prompt; only a process outside the active desktop session (session 0, a remote non-admin session) is told to use an elevated terminal (found live on 2026-10-03: install --host through ssh into WSL was refused although session 1 could prompt)"
bound = false

[[acceptance]]
text = "Given goway add or goway doctor --fix with --rsudo for a step that needs Windows administrator rights on a helper, when it runs, then goway elevates on the helper through its Windows OpenSSH server as an administrator account (key in administrators_authorized_keys, unattended) if one is configured, else through goway-setup and a UAC prompt on the helper's desktop when someone is logged in there, else stops with the exact command to run as administrator on the helper; no password is ever stored or typed into goway, and the elevated session never starts WSL"
bound = false

[[acceptance]]
text = "Given --lsudo for a Windows-side step on the main laptop, when it runs, then the local UAC prompt is used"
bound = false
+++
