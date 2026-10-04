+++
id = "01M44CX1SDVT5JW2N1R893RAE4"
title = "Audit3 M1: repository package names cannot remove packages or act as package manager modifiers"
type = "security"
category = "in-progress"
priority = "medium"
points = 2
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-04T21:25:15Z"
updated = "2026-10-04T21:33:18Z"
labels = ["security"]
scope = ["crates/goway/src/doctor/prereq.rs", "docs/usage.md"]

[[acceptance]]
text = "Given a goway.toml package name such as sudo-, curl+, a=1 or a/b, when doctor validates it, then it is rejected for apt, dnf and pacman"
bound = true

[[acceptance]]
text = "Given a valid package name, when doctor builds the root fix, then names follow -- and apt gets --no-remove and pacman --needed"
bound = true
+++
