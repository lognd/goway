+++
id = "01M41J9F9X5EXN9SS579WA8S1D"
title = "Public release: GitHub repo, CI on Linux and Windows, release artifacts and one-line installers"
type = "task"
category = "todo"
priority = "medium"
points = 5
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-03T19:01:41Z"
updated = "2026-10-03T19:01:41Z"
labels = ["newcomer"]
scope = [".github/**"]

[[acceptance]]
text = "Given a version tag, when CI runs, then the release has Linux x86_64 and aarch64 binaries, goway-setup.exe for x64 and arm64, sha256 sums, and install.sh"
bound = false

[[acceptance]]
text = "Given a fresh Linux or WSL machine, when the user runs the documented one-line installer, then goway is installed per user and verified against the published sha256"
bound = false
+++
