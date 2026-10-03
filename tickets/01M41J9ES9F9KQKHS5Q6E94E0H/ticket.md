+++
id = "01M41J9ES9F9KQKHS5Q6E94E0H"
title = "goway uninstall: remove everything goway added, here and on every helper"
type = "story"
category = "todo"
priority = "medium"
points = 5
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-03T19:01:41Z"
updated = "2026-10-03T19:01:41Z"
labels = ["newcomer"]
scope = ["crates/goway/**"]

[[acceptance]]
text = "Given a laptop with goway set up for helpers, when goway uninstall --everywhere runs, then each helper loses goway's remote state, the authorized key line and the tools goway installed, and this laptop loses goway's config, state and binary"
bound = false

[[acceptance]]
text = "Given goway uninstall without --everywhere, when it runs, then it lists exactly what it would remove on each machine and asks before removing"
bound = false
+++
