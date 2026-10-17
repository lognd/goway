+++
id = "01M54EQ3CGJEFP5BT3TSG8MFPH"
title = "goway-setup --join leftovers: end-to-end native join test and goway.exe on PATH in the distro"
type = "task"
category = "in-progress"
priority = "medium"
points = 3
parent = "01M54433AG8J1V0GYT3ZWY9C9A"
reporter = "lognd"
created = "2026-10-17T08:12:39Z"
updated = "2026-10-17T09:42:19Z"
scope = ["crates/goway-setup/**"]

[[acceptance]]
text = "native join runs end to end against a real child process standing in for goway.exe (card, join, access)"
bound = false

[[acceptance]]
text = "the WSL join links goway.exe into the distro PATH when goway is missing and never replaces an existing one"
bound = false
+++

leftover from ~M7H9TBV: the cli glue (join_native, show_card) is covered only through HostSystem unit tests; the WSL join still needs goway on the distro PATH and the installer does not put it there; native join runs as the invoking user, not the sshd account if different
