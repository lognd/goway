+++
id = "01M41QK0N9EG1DA8BZFB7ANNPX"
title = "Host-derived text cannot forge goway lines; helper output is size-capped"
type = "security"
category = "in-progress"
priority = "medium"
points = 3
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-03T20:34:17Z"
updated = "2026-10-03T20:34:20Z"
scope = ["crates/goway/src/render.rs", "crates/goway/src/sync.rs", "crates/goway/src/remotesys.rs", "crates/goway/src/remote.rs", "docs/troubleshooting.md", "SECURITY.md"]

[[acceptance]]
text = "Given host-derived text with a second line starting with goway:, When it is printed through the renderer, Then no output line starts with goway: except the real one"
bound = false

[[acceptance]]
text = "Given host-derived text with bidi or zero-width format characters or huge length, When it is cleaned, Then they are replaced and the length is capped"
bound = false

[[acceptance]]
text = "Given a helper that prints more than the cap on stdout or stderr, When goway captures it, Then memory stays bounded, stderr is truncated and oversize stdout is an error"
bound = false
+++
