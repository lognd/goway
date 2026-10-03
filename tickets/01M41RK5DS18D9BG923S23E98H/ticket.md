+++
id = "01M41RK5DS18D9BG923S23E98H"
title = "Windows installer: elevate a locked, hashed private copy of the setup exe (audit F6)"
type = "security"
category = "in-progress"
priority = "medium"
points = 3
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-03T20:51:50Z"
updated = "2026-10-03T20:54:59Z"
scope = ["crates/goway-setup/**", "crates/goway-journal/**", "docs/**", "scripts/windows/**", "changelog.d/**"]

[[acceptance]]
text = "Given the setup exe in a user-writable folder, when the install asks UAC for administrator rights, then the elevated process is a private copy held with a share mode that denies writing, renaming and deleting, and the elevated side refuses an image whose SHA-256 differs from the one its parent locked"
bound = true
+++
