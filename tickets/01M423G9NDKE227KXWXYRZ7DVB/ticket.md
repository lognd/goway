+++
id = "01M423G9NDKE227KXWXYRZ7DVB"
title = "README: direct download links for every release file, with how to verify them; a test keeps the names in step with the release workflow"
type = "docs"
category = "done"
outcome = "done"
priority = "high"
points = 2
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-04T00:02:31Z"
updated = "2026-10-04T00:06:29Z"
scope = ["README.md", "crates/goway/tests/publishing.rs"]

[[acceptance]]
text = "Given README.md, when a reader wants goway without a terminal one-liner, then a download table links each release file directly through releases/latest/download (goway-setup.exe, goway-setup-arm64.exe, the four goway tar.gz archives, install.sh, SHA256SUMS), says which computer each is for, and a collapsed section explains how to verify a download (sha256sum or Get-FileHash against SHA256SUMS, and gh attestation verify)"
bound = true

[[acceptance]]
text = "Given the release workflow, when a test compares every releases/latest/download link in README.md with the files the workflow puts in dist, then every linked file is one the release publishes"
bound = true
+++
