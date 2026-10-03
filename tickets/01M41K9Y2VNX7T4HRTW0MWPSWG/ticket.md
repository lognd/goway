+++
id = "01M41K9Y2VNX7T4HRTW0MWPSWG"
title = "Screen-reader friendly output: --plain renders tables as labelled lines"
type = "task"
category = "done"
outcome = "done"
priority = "medium"
points = 1
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-03T19:19:25Z"
updated = "2026-10-03T19:21:28Z"
labels = ["newcomer"]
scope = ["crates/goway/**", "README.md"]

[[acceptance]]
text = "Given goway --plain status (or GOWAY_PLAIN=1), when it prints, then each row is one line of 'label: value' pairs instead of space-aligned columns"
bound = true
+++
