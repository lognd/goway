+++
id = "01M41G4977R6YQJNT25JXH361J"
title = "M3: gc removes only labelled entries under a marked root; remote_root validated"
type = "security"
category = "done"
outcome = "done"
priority = "high"
points = 2
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-03T18:23:54Z"
updated = "2026-10-03T18:35:48Z"
labels = ["security"]
scope = ["crates/goway/**", "docs/usage.md", "docs/config.md"]

[[acceptance]]
text = "Given remote_root set to . or an unlabelled old directory under the root, when goway gc runs, then the config is rejected or the unlabelled directory is kept"
bound = true
+++
