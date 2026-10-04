+++
id = "01M44DCY5N2R7KWCPZ9169R3Q0"
title = "Audit3 M2 and L1: one PowerShell quoter that handles curly quotes, and printed admin commands without expansion"
type = "security"
category = "in-progress"
priority = "medium"
points = 3
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-04T21:33:55Z"
updated = "2026-10-04T21:40:03Z"
labels = ["security"]
scope = ["crates/goway-journal/src/lib.rs", "crates/goway-journal/src/psquote.rs", "crates/goway/src/transport.rs", "crates/goway-setup/src/ps.rs", "crates/goway-setup/src/helper.rs", "crates/goway/src/sshsetup.rs", "crates/goway/tests/ps_quote.rs", "crates/goway-setup/tests/hostsys.rs", "docs/design.md"]

[[acceptance]]
text = "Given any string including U+2018 to U+201B, quotes, dollar signs and newlines, when it is quoted for PowerShell and evaluated by pwsh, then it round-trips exactly with no side effect"
bound = true

[[acceptance]]
text = "Given a key comment containing dollar-parenthesis, backtick or curly quotes, when native_setup_command prints the administrator command, then the comment is absent and the key is a single-quoted literal"
bound = true
+++
