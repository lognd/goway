+++
id = "01M41Q6CX4RRFZT58EE995C0C6"
title = "Host kinds: windows over interop (powershell.exe, no ssh) and windows over OpenSSH, behind one transport"
type = "story"
category = "in-progress"
priority = "medium"
points = 3
parent = "01M41Q6CAC5EW88V3ZFSFJ1T6J"
reporter = "lognd"
created = "2026-10-03T20:27:23Z"
updated = "2026-10-03T22:48:46Z"
scope = ["crates/goway/src/config.rs", "crates/goway/src/ssh.rs", "crates/goway/src/transport.rs", "crates/goway/src/interop.rs", "crates/goway/src/hosts.rs", "crates/goway/tests/**", "docs/config.md", "docs/hosts.md", "crates/goway/src/local.rs", "crates/goway/src/pool.rs", "crates/goway/src/resolve.rs", "crates/goway/src/sshsetup.rs", "crates/goway/src/status.rs", "crates/goway/src/needs.rs"]

[[acceptance]]
text = "Given a [[host]] with os = windows and transport = interop, when goway runs on it from WSL, then it reaches the local Windows through powershell.exe with no ssh and no network listener"
bound = false

[[acceptance]]
text = "Given a [[host]] with os = windows over ssh, when goway runs on it, then it uses OpenSSH with PowerShell as the remote shell and the same pinned-key rules as other hosts"
bound = false

[[acceptance]]
text = "Given goway add for a Windows machine, when it runs, then it detects Windows, records the kind, and never confuses it with that machine's WSL host"
bound = false
+++
