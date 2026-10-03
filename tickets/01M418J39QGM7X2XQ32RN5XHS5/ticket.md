+++
id = "01M418J39QGM7X2XQ32RN5XHS5"
title = "Windows installer: host component (firewall, Hyper-V firewall, keepalive, wslconfig, WSL sshd)"
type = "task"
category = "todo"
priority = "medium"
points = 8
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-03T16:11:38Z"
updated = "2026-10-03T16:11:38Z"
scope = ["crates/goway-setup/**", "docs/**"]

[[acceptance]]
text = "Given a Windows host, when goway-setup install --host then uninstall run in a test profile, then firewall rules, scheduled tasks, .wslconfig and WSL sshd config equal the snapshot from before"
bound = false
+++
