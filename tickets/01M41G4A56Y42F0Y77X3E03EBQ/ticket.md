+++
id = "01M41G4A56Y42F0Y77X3E03EBQ"
title = "M7: broader secret denylist, case-insensitive, no files under symlinked parents"
type = "security"
category = "done"
outcome = "done"
priority = "high"
points = 2
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-03T18:23:55Z"
updated = "2026-10-03T18:50:25Z"
labels = ["security"]
scope = ["crates/goway/**", "docs/usage.md", "docs/config.md", "docs/positioning.md"]

[[acceptance]]
text = "Given a work tree with .ENV, .envrc, .npmrc, id_rsa and a file under a symlinked directory, when goway computes the file set, then none of them is sent unless explicitly allowed"
bound = true
+++
