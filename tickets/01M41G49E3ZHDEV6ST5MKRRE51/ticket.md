+++
id = "01M41G49E3ZHDEV6ST5MKRRE51"
title = "M4: sanitize remote-originated text before rendering; reject non-finite probe values"
type = "security"
category = "todo"
priority = "high"
points = 2
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-03T18:23:54Z"
updated = "2026-10-03T18:23:54Z"
labels = ["security"]
scope = ["crates/goway/**"]

[[acceptance]]
text = "Given a host whose hostname contains terminal escape sequences or whose load is -inf, when goway status or run prints it or picks a host, then no control characters reach the terminal and the host is not preferred"
bound = false
+++
