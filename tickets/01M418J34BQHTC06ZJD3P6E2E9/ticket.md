+++
id = "01M418J34BQHTC06ZJD3P6E2E9"
title = "Guided, reversible ssh setup: keys, authorized_keys, permissions, icacls"
type = "task"
category = "todo"
priority = "medium"
points = 5
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-03T16:11:38Z"
updated = "2026-10-03T16:11:38Z"
scope = ["crates/**", "docs/**"]

[[acceptance]]
text = "Given a host, when goway ssh setup runs and then goway ssh setup --undo runs, then authorized_keys and local files equal their state before setup"
bound = false

[[acceptance]]
text = "Given a Windows sshd target, when setup writes administrators_authorized_keys, then the ACL grants only Administrators and SYSTEM"
bound = false
+++
