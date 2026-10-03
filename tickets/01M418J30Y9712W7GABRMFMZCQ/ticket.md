+++
id = "01M418J30Y9712W7GABRMFMZCQ"
title = "goway status: hosts, load, jobs and disk"
type = "task"
category = "in-progress"
priority = "medium"
points = 2
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-03T16:11:38Z"
updated = "2026-10-03T16:53:30Z"
scope = ["crates/goway/**"]

[[acceptance]]
text = "Given configured hosts, when goway status runs, then each host shows address, cores, load, running jobs and disk used, unreachable ones marked"
bound = true
+++
