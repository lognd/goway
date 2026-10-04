+++
id = "01M42BHT6R8A6Q9HK40VK9Z2AX"
title = "Logout must not kill goway: detect KillUserProcesses=yes and missing lingering on Linux helpers"
type = "story"
category = "done"
outcome = "done"
priority = "medium"
points = 1
parent = "01M42BHPTWVNZ4DZ0H4GXG60MX"
reporter = "lognd"
created = "2026-10-04T02:23:09Z"
updated = "2026-10-04T05:44:57Z"
scope = ["crates/goway/src/doctor.rs", "docs/troubleshooting.md", "crates/goway/src/doctor/logout.rs", "crates/goway/src/doctor/prereq.rs"]

[[acceptance]]
text = "Given a Linux helper whose logind kills user processes at logout (KillUserProcesses=yes) or an encrypted or network home that is unmounted when nobody is logged in, when doctor runs, then it reports the risk and the fix (loginctl enable-linger for the user, or a remote_root outside the encrypted home) through --rsudo where root is needed"
bound = true
+++
