+++
id = "01M418J2MCP8Y962617FMZKNKR"
title = "Workspace skeleton: CLI, render module, tracing, error and exit-code types"
type = "task"
category = "in-progress"
priority = "medium"
points = 3
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-03T16:11:37Z"
updated = "2026-10-03T16:11:55Z"
scope = ["crates/goway/**", "docs/**", "Cargo.toml"]

[[acceptance]]
text = "Given the workspace, when cargo clippy runs, then print macros outside render.rs are denied"
bound = false

[[acceptance]]
text = "Given any goway failure, when goway exits, then the exit code is 125 and the error is rendered on stderr"
bound = false
+++
