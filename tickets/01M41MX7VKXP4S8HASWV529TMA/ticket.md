+++
id = "01M41MX7VKXP4S8HASWV529TMA"
title = "Helpers on Windows 10 / WSL NAT mode: journaled portproxy relay kept current by the keepalive task"
type = "story"
category = "todo"
priority = "medium"
points = 5
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-03T19:47:26Z"
updated = "2026-10-03T19:47:26Z"
scope = ["crates/goway-setup/**", "docs/install-windows.md", "README.md"]

[[acceptance]]
text = "Given a helper whose WSL runs in NAT mode (Windows 10, or Windows 11 without mirrored networking), when goway-setup install --host runs, then other computers on the local network reach its WSL sshd through a Windows port relay scoped like the firewall rule"
bound = false

[[acceptance]]
text = "Given WSL restarts and its internal address changes, when the keepalive task runs, then the relay points at the new address"
bound = false

[[acceptance]]
text = "Given goway-setup uninstall --host, when it runs, then the relay and everything else the install added are removed exactly"
bound = false
+++
