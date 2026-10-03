+++
id = "01M41J9EDG2P620PVQSSZ2EHXD"
title = "Accessible output: words not colors, a next step on every error"
type = "task"
category = "in-progress"
priority = "medium"
points = 2
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-03T19:01:40Z"
updated = "2026-10-03T19:01:59Z"
labels = ["newcomer"]
scope = ["crates/goway/src/**", "crates/goway/tests/**"]

[[acceptance]]
text = "Given any goway message, when color is off, then its kind (ok, note, warning, error, next) is a word in the line"
bound = false

[[acceptance]]
text = "Given the errors unreachable host, key refused, host key mismatch, not a git repository and no usable host, when they are shown, then each names the next step to take"
bound = false
+++
