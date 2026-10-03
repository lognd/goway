+++
id = "01M418J2SVQ1TDSVW4Y3K8SFYY"
title = "Sync work tree to a remote seed mirror with tar deltas"
type = "task"
category = "todo"
priority = "medium"
points = 5
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-03T16:11:38Z"
updated = "2026-10-03T16:25:18Z"
scope = ["crates/goway/**"]

[[acceptance]]
text = "Given a work tree with tracked, modified, untracked and ignored files and a .env, when goway computes the file set, then it equals git ls-files -co --exclude-standard minus deleted files and .env files"
bound = false

[[acceptance]]
text = "Given a remote manifest, when goway diffs it, then only files with changed size or mtime are sent and files gone locally are deleted"
bound = false
+++
