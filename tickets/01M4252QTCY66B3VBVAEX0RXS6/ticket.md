+++
id = "01M4252QTCY66B3VBVAEX0RXS6"
title = "Prove goway-setup install --host --native on a spare Windows machine: roundtrip snapshot in a test profile"
type = "task"
category = "todo"
priority = "medium"
points = 2
reporter = "lognd"
created = "2026-10-04T00:30:04Z"
updated = "2026-10-04T00:30:25Z"
scope = ["scripts/windows/roundtrip-host.sh", "docs/install-windows.md"]

[[acceptance]]
text = "Given a Windows machine that can be spared (no hand-made OpenSSH setup in use), when goway-setup install --host --native then uninstall run in the goway-test profile, then before and after snapshots of the capability, sshd service, firewall rules, HKLM OpenSSH key and key files are identical; found while working ~VHRF97S, whose PowerShell scripts are only tested through a fake runner"
bound = false
+++
