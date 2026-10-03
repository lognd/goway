+++
id = "01M418J37YMHXD44VH1PPMS0PF"
title = "Windows installer: client component (goway.exe, user PATH, uninstall entry)"
type = "task"
category = "in-progress"
priority = "medium"
points = 5
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-03T16:11:38Z"
updated = "2026-10-03T16:41:23Z"
scope = ["crates/goway-setup/**", "docs/install-windows.md", "scripts/windows/**", "crates/goway-journal/**", "frob.toml"]

[[acceptance]]
text = "Given a Windows host, when goway-setup install --client then uninstall run in a test profile, then the snapshot of user PATH, uninstall keys and files is identical to before"
bound = false
+++
