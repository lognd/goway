+++
id = "01M41DJE8ZZJXCHY01NKK754K3"
title = "Test goway host add end to end (port fallback, Linux and hostname checks, key pinning)"
type = "task"
category = "todo"
priority = "medium"
points = 1
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-03T17:39:12Z"
updated = "2026-10-03T17:39:12Z"
scope = ["crates/goway/tests/**"]

[[acceptance]]
text = "Given a fake sshd, when goway host add runs, then it skips a non-Linux port, refuses a machine whose hostname does not match, and pins the key of the right machine under goway-NAME"
bound = false
+++
