+++
id = "01M41Q5QV6WPG98DFF5WMMTNRW"
title = "Helper next-steps: validate and quote the WSL user, clean system text in the setup renderer"
type = "security"
category = "todo"
priority = "medium"
points = 2
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-03T20:27:02Z"
updated = "2026-10-03T20:27:02Z"
scope = ["crates/goway-setup/**"]

[[acceptance]]
text = "Given a WSL default user name that is not a valid Linux user name, when the helper block or status --host is printed, then the name never appears in the add command line and a warning is shown"
bound = false

[[acceptance]]
text = "Given system-derived text with control characters, when goway-setup renders it, then the control and bidi characters are stripped"
bound = false
+++
