+++
id = "01M41P2G171VZJW77Z1XP4RZVS"
title = "doctor checks the toolchains a project needs (Node, Python, Java, C/C++, Go, Ruby, .NET)"
type = "story"
category = "todo"
priority = "medium"
points = 5
parent = "01M41P2FPBSV4WFSQDGQY7SC89"
reporter = "lognd"
created = "2026-10-03T20:07:47Z"
updated = "2026-10-03T20:07:47Z"
scope = ["crates/goway/src/doctor.rs", "crates/goway/src/remote.sh", "crates/goway/tests/**", "docs/usage.md"]

[[acceptance]]
text = "Given a project folder, when goway doctor runs there, then it detects the ecosystems from the project's files and checks each host for their tools, with exact fixes (user-level where possible, root fixes through --rsudo)"
bound = false
+++
