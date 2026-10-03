+++
id = "01M41AV1FXKNJQD47M4YCXEWYB"
title = "Coexistence contract: never override other build systems' settings"
type = "task"
category = "in-progress"
priority = "medium"
points = 2
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-03T16:51:28Z"
updated = "2026-10-03T17:06:44Z"
scope = ["crates/goway/**", "docs/positioning.md", "docs/config.md"]

[[acceptance]]
text = "Given RUSTC_WRAPPER, CARGO_TARGET_DIR or SCCACHE_* set through --env or the remote environment, when goway runs a job, then the user's values win over goway's defaults"
bound = false

[[acceptance]]
text = "Given a goway run, when it finishes, then goway left no file outside its remote root and no process behind"
bound = false
+++
