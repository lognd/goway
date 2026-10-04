+++
id = "01M43JBGHCD68QDZVMDMG5P4GS"
title = "Allow max_disk, min_free and cache_size per [[host]], overriding [defaults]"
type = "story"
category = "todo"
priority = "medium"
points = 1
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-04T13:41:17Z"
updated = "2026-10-04T13:41:17Z"
scope = ["crates/goway/src/config.rs", "docs/config.md"]

[[acceptance]]
text = "Given max_disk, min_free or cache_size on a [[host]], when goway computes that host's disk budget, then the host's values override [defaults] (a small helper can keep a small budget while big ones get a large one)"
bound = false
+++
