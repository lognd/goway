+++
id = "01M41J9EJQR571J5GCSWYXXE81"
title = "goway add HOST: one command registers a helper (key, pin, toolchain), with --rsudo/--lsudo"
type = "story"
category = "in-progress"
priority = "medium"
points = 5
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-03T19:01:41Z"
updated = "2026-10-03T19:08:29Z"
labels = ["newcomer"]
scope = ["crates/goway/**", "docs/usage.md", "docs/ssh-setup.md"]

[[acceptance]]
text = "Given a helper that answers on ssh, when goway add HELPER --fingerprint SHA256:x runs, then key login works, the host is pinned and in the pool, and its user-level toolchain is installed, with one password prompt"
bound = true

[[acceptance]]
text = "Given root fixes are needed, when goway add HELPER --rsudo runs, then they run in one interactive sudo session on the helper after one confirmation, and without --rsudo they are only listed with reasons"
bound = false

[[acceptance]]
text = "Given goway add HELPER runs again, when the helper is already set up, then it changes nothing and says so"
bound = true
+++
