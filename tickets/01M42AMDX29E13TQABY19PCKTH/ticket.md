+++
id = "01M42AMDX29E13TQABY19PCKTH"
title = "rsudo and lsudo elevate Windows-side steps through an admin OpenSSH session or UAC, never with a stored password"
type = "story"
category = "in-progress"
priority = "high"
points = 5
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-04T02:07:06Z"
updated = "2026-10-04T04:10:58Z"
scope = ["crates/goway/src/add.rs", "crates/goway/src/doctor.rs", "crates/goway/src/cli.rs", "docs/usage.md", "docs/install-windows.md", "crates/goway/src/sshsetup.rs", "crates/goway/src/winadmin.rs", "crates/goway/src/lib.rs", "crates/goway/tests/win_elevate.rs"]

[[acceptance]]
text = "Given goway add or goway doctor --fix with --rsudo for a step that needs Windows administrator rights on a helper, when it runs, then goway elevates on the helper through its Windows OpenSSH server as an administrator account (key in administrators_authorized_keys, unattended) if one is configured, else through goway-setup and a UAC prompt on the helper's desktop when someone is logged in there, else stops with the exact command to run as administrator on the helper; no password is ever stored or typed into goway, and the elevated session never starts WSL"
bound = true

[[acceptance]]
text = "Given --lsudo for a Windows-side step on the main laptop, when it runs, then the local UAC prompt is used"
bound = true
+++

split from ~AD8JP8D (criteria 2 and 3) by lane C
