+++
id = "01M42BHTN4T6FJ9DDR3E51QN55"
title = "Toolchains that only appear in login shells, and helpers behind an HTTP proxy"
type = "story"
category = "in-progress"
priority = "medium"
points = 1
parent = "01M42BHPTWVNZ4DZ0H4GXG60MX"
reporter = "lognd"
created = "2026-10-04T02:23:09Z"
updated = "2026-10-04T05:55:35Z"
scope = ["crates/goway/src/remote.sh", "docs/troubleshooting.md", "crates/goway/tests/user_tool_path.rs", "docs/config.md", "docs/design.md", "docs/positioning.md"]

[[acceptance]]
text = "Given rustup, uv, node, go or mold installed for login shells or per-user directories only (PATH set in .profile, not for ssh commands), when goway runs or doctor checks, then goway finds the usual per-user tool locations without sourcing startup files, and the host facts name proxy variables without their values"
bound = true
+++
