+++
id = "01M41DQSPQB08SS2XZ0EMGSRZ5"
title = "Exercise the interactive UAC elevation path of goway-setup install --host"
type = "task"
category = "todo"
priority = "medium"
points = 1
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-03T17:42:08Z"
updated = "2026-10-04T01:59:52Z"
scope = ["crates/goway-setup/**"]

[[acceptance]]
text = "Given an ordinary (non-elevated) console on Windows, when goway-setup install --host runs, then it relaunches elevated through UAC and the journal records the same changes as an elevated run"
bound = false
+++
