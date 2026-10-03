+++
id = "01M41AV1JM5RCN804G0D4TJVJS"
title = "Shard one test run across hosts with nextest --partition"
type = "story"
category = "done"
outcome = "done"
priority = "medium"
points = 5
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-03T16:51:28Z"
updated = "2026-10-03T17:39:02Z"
scope = ["crates/goway/**", "docs/usage.md"]

[[acceptance]]
text = "Given goway run --shard N -- cargo nextest run, when it runs, then N hosts each run partition i/N and goway exits non-zero if any shard fails"
bound = true
+++
