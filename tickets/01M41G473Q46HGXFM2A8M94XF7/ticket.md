+++
id = "01M41G473Q46HGXFM2A8M94XF7"
title = "H1: elevated host install/uninstall must not trust user-writable journal, settings or binaries"
type = "security"
category = "in-progress"
priority = "critical"
points = 8
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-03T18:23:52Z"
updated = "2026-10-03T18:48:58Z"
labels = ["security"]
scope = ["crates/goway-setup/**", "crates/goway-journal/**", "scripts/windows/**", "docs/install-windows.md"]

[[acceptance]]
text = "Given a journal, settings file or setup exe planted by a non-admin process, when the elevated host install or uninstall runs, then it refuses to act on it (state lives in an admin-only directory, entries are validated against the expected host plan, system tools are called by absolute path, and only the host component elevates)"
bound = true
+++
