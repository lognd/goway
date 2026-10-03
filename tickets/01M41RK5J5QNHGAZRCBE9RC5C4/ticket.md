+++
id = "01M41RK5J5QNHGAZRCBE9RC5C4"
title = "Windows installer: the elevated child never follows links in user files and validates journal priors (audit L9)"
type = "security"
category = "done"
outcome = "done"
priority = "medium"
points = 3
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-03T20:51:50Z"
updated = "2026-10-03T20:58:18Z"
scope = ["crates/goway-setup/**", "crates/goway-journal/**", "docs/**", "scripts/windows/**", "changelog.d/**"]

[[acceptance]]
text = "Given .wslconfig is a symbolic link or reparse point, when the elevated child edits or reverts it, then it refuses and writes nothing through the link"
bound = true

[[acceptance]]
text = "Given a host journal whose recorded prior does not fit its change, when the elevated uninstall validates it, then it is refused before anything is reverted"
bound = true
+++
