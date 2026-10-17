+++
id = "01M55X73AV6A85Q1N1MS2V4C26"
title = "Audit L7: circle marker in authorized_keys needs genesis id"
type = "bug"
category = "in-progress"
priority = "medium"
points = 2
reporter = "lognd"
created = "2026-10-17T21:45:17Z"
updated = "2026-10-17T21:47:47Z"
scope = ["crates/goway-circle/src/state.rs", "crates/goway-circle/src/sshcert.rs", "changelog.d/01M55X73AV6A85Q1N1MS2V4C26.fixed.md"]

[[acceptance]]
text = "Two same-named circles keep separate authorized_keys lines; old markers still replaced"
bound = true
+++

found while working N5H9JF3: State has no genesis id so line_marker cannot embed it; add State.genesis, use goway-circle:<id>:<name>:e<epoch>, keep matching the old form in merge_authorized_keys.
