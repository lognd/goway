+++
id = "01M41RK5G48AYWK5ATTNFFPK09"
title = "Windows installer: refuse wide and any-like --allow-from ranges unless --allow-wide (audit L4)"
type = "security"
category = "in-progress"
priority = "medium"
points = 2
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-03T20:51:50Z"
updated = "2026-10-03T20:55:04Z"
scope = ["crates/goway-setup/**", "crates/goway-journal/**", "docs/**", "scripts/windows/**", "changelog.d/**"]

[[acceptance]]
text = "Given an --allow-from range shorter than /8 (IPv4) or /16 (IPv6), when install runs without --allow-wide, then it is refused"
bound = false

[[acceptance]]
text = "Given a range that contains 0.0.0.0, :: or multicast space, when install runs with or without --allow-wide, then it is refused"
bound = false
+++
