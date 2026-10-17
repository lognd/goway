+++
id = "01M5493X71AKND6PKE8WNW4H3Y"
title = "OS keystore integration for the circle node and admin keys"
type = "task"
category = "in-progress"
priority = "medium"
points = 3
reporter = "lognd"
created = "2026-10-17T06:34:47Z"
updated = "2026-10-17T12:03:53Z"
scope = ["crates/goway-circle/**", "docs/circles.md"]

[[acceptance]]
text = "A saved keystore passphrase is returned without prompting"
bound = false

[[acceptance]]
text = "A prompted passphrase is remembered in the keystore"
bound = false

[[acceptance]]
text = "A failing keystore falls back to the prompt"
bound = false
+++

found while working ~MV967CF; deferred hardening, see docs/circles.md section 6
