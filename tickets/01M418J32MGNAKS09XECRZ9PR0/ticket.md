+++
id = "01M418J32MGNAKS09XECRZ9PR0"
title = "goway doctor with --fix: toolchain and ssh checks, sudo steps explained"
type = "task"
category = "todo"
priority = "medium"
points = 3
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-03T16:11:38Z"
updated = "2026-10-03T16:11:38Z"
scope = ["crates/goway/**", "docs/**"]

[[acceptance]]
text = "Given a host without cargo-nextest, when goway doctor runs, then it reports the missing tool with the exact fix command"
bound = false

[[acceptance]]
text = "Given a fix that needs root, when goway doctor --fix runs, then it does not run it and tells the user to run it with sudo and why"
bound = false
+++
