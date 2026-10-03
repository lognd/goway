+++
id = "01M41DQSJZS28G72PJWFMD718W"
title = "Owner awareness: skip a host while its user is active or it runs on battery"
type = "task"
category = "todo"
priority = "medium"
points = 3
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-03T17:42:08Z"
updated = "2026-10-03T17:42:08Z"
scope = ["crates/goway/**"]

[[acceptance]]
text = "Given a host whose Windows user was active within the configured idle window or which runs on battery, when goway picks a host, then that host is skipped unless pinned"
bound = false
+++
