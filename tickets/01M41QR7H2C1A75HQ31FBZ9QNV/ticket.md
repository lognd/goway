+++
id = "01M41QR7H2C1A75HQ31FBZ9QNV"
title = "install.sh runs only when fully downloaded, uses https only, appends to PATH and heals a stuck journal"
type = "security"
category = "in-progress"
priority = "medium"
points = 3
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-03T20:37:08Z"
updated = "2026-10-03T20:38:56Z"
scope = ["scripts/install.sh", "crates/goway/tests/install_scripts.rs", "docs/install-linux.md"]

[[acceptance]]
text = "Given install.sh cut off at any line, When it is run, Then it either completes or leaves no state"
bound = true

[[acceptance]]
text = "Given GOWAY_RELEASE_URL with http://, When install.sh runs, Then it refuses"
bound = false

[[acceptance]]
text = "Given a journal left by an aborted install with no binary, When install.sh runs, Then it clears the stale journal and installs"
bound = false

[[acceptance]]
text = "Given install.sh adds the PATH line, When ~/.profile is read, Then the goway bin directory is appended after the existing PATH"
bound = false
+++
