+++
id = "01M41J9F45EY1FPVVV9HDJ5HFB"
title = "Helper laptop one command: install --host prints the fingerprint and the exact next command, owns its uninstall entry"
type = "story"
category = "in-progress"
priority = "medium"
points = 3
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-03T19:01:41Z"
updated = "2026-10-03T19:05:12Z"
labels = ["newcomer"]
scope = ["crates/goway-setup/**", "docs/install-windows.md", "scripts/windows/**", "crates/goway-journal/**"]

[[acceptance]]
text = "Given a Windows laptop with WSL, when goway-setup.exe install --host finishes, then it prints the host key fingerprint and the exact goway add command to run on the main laptop"
bound = false

[[acceptance]]
text = "Given a host-only install, when it finishes, then Add/Remove Programs lists it and uninstall works without the downloaded file"
bound = false

[[acceptance]]
text = "Given a Windows laptop without WSL, when install --host runs, then it stops before changing anything and prints the exact steps to install WSL"
bound = false
+++
