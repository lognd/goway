+++
id = "01M41J9F9X5EXN9SS579WA8S1D"
title = "Public release: GitHub repo, CI on Linux and Windows, release artifacts and one-line installers"
type = "task"
category = "in-progress"
priority = "medium"
points = 5
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-03T19:01:41Z"
updated = "2026-10-03T19:25:00Z"
labels = ["newcomer"]
scope = [".github/**", "scripts/install.sh", "crates/goway/tests/install_scripts.rs", "docs/install-linux.md", "docs/release.md"]

[[acceptance]]
text = "Given a fresh Linux or WSL machine, when the user runs the documented one-line installer, then goway is installed per user and verified against the published sha256"
bound = true

[[acceptance]]
text = "Given .github/workflows/ci.yml and release.yml, when they are read, then they define CI on push and a tag-triggered release of Linux x86_64/aarch64 binaries, goway-setup.exe x64/arm64, SHA256SUMS, install.sh and provenance attestations"
bound = false
+++
