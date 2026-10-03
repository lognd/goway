+++
id = "01M41P2FTWWZGR0Q5JTNW5SF9B"
title = "macOS main laptop: release binaries and install.sh for Apple Silicon and Intel"
type = "story"
category = "in-progress"
priority = "medium"
points = 3
parent = "01M41P2FPBSV4WFSQDGQY7SC89"
reporter = "lognd"
created = "2026-10-03T20:07:47Z"
updated = "2026-10-03T21:32:23Z"
scope = ["scripts/install.sh", ".github/workflows/release.yml", "crates/goway/tests/install_scripts.rs", "docs/release.md", "scripts/uninstall.sh", ".github/workflows/ci.yml", "README.md", "crates/goway/src/sync.rs"]

[[acceptance]]
text = "Given a version tag, when the release workflow runs, then it publishes static-enough goway binaries for aarch64-apple-darwin and x86_64-apple-darwin with checksums"
bound = true

[[acceptance]]
text = "Given macOS, when the one-line installer runs, then it downloads the right binary, verifies it with shasum, and installs it"
bound = true
+++
