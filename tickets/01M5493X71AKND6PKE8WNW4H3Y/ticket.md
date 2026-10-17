+++
id = "01M5493X71AKND6PKE8WNW4H3Y"
title = "OS keystore integration for the circle node and admin keys"
type = "task"
category = "done"
outcome = "done"
priority = "medium"
points = 3
reporter = "lognd"
created = "2026-10-17T06:34:47Z"
updated = "2026-10-17T12:04:08Z"
scope = ["crates/goway-circle/**", "docs/circles.md", "changelog.d/01M5493X71AKND6PKE8WNW4H3Y.added.md"]

[[acceptance]]
text = "A saved keystore passphrase is returned without prompting"
bound = true

[[acceptance]]
text = "A prompted passphrase is remembered in the keystore"
bound = true

[[acceptance]]
text = "A failing keystore falls back to the prompt"
bound = true
+++

found while working ~MV967CF; deferred hardening, see docs/circles.md section 6
