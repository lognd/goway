+++
id = "01M41G48ZSY788REK7AV39HJ02"
title = "M2: run work trees must not share inodes with seeds or other runs"
type = "security"
category = "in-progress"
priority = "high"
points = 2
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-03T18:23:54Z"
updated = "2026-10-03T18:34:06Z"
labels = ["security"]
scope = ["crates/goway/**", "docs/usage.md", "docs/design.md"]

[[acceptance]]
text = "Given a job that appends to a tracked file in place, when it runs, then the seed, sibling seeds and other runs keep their content"
bound = true
+++
