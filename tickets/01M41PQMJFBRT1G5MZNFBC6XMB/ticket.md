+++
id = "01M41PQMJFBRT1G5MZNFBC6XMB"
title = "Filter terminal control sequences from remote output when stdout or stderr is a terminal"
type = "security"
category = "in-progress"
priority = "medium"
points = 5
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-03T20:19:20Z"
updated = "2026-10-03T20:23:48Z"
scope = ["crates/goway/src/**", "crates/goway/tests/**", "SECURITY.md", "docs/usage.md", "docs/design.md"]

[[acceptance]]
text = "Given a terminal stream, When remote output holds OSC, DCS, APC, PM, SOS, non-SGR CSI or other control bytes, Then only SGR, text, newline, CR, tab and backspace reach the terminal, even when a sequence is split across reads"
bound = true

[[acceptance]]
text = "Given --output=raw or GOWAY_OUTPUT=raw, When the stream is a terminal, Then bytes pass through untouched"
bound = false

[[acceptance]]
text = "Given stdout is a pipe or file, When remote output holds escape sequences, Then the bytes arrive identical"
bound = false
+++
