+++
id = "01M41G4AMWANFMSM4N616W3VKV"
title = "M10: separate caches for forks sharing a root commit; document same-user execution"
type = "security"
category = "done"
outcome = "done"
priority = "medium"
points = 2
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-03T18:23:56Z"
updated = "2026-10-03T18:54:33Z"
labels = ["security"]
scope = ["crates/goway/**", "docs/positioning.md", "docs/usage.md"]

[[acceptance]]
text = "Given two repositories with the same root commit but different origin URLs, when goway runs them, then they use different remote caches"
bound = true
+++
