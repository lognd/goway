+++
id = "01M418J2VJ0VQANJXMWZWZN75P"
title = "goway run: per-run work dir, cargo target slots, streaming, faithful exit code, cancel"
type = "task"
category = "in-progress"
priority = "medium"
points = 8
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-03T16:11:38Z"
updated = "2026-10-03T16:47:21Z"
scope = ["crates/goway/**"]

[[acceptance]]
text = "Given goway run -- sh -c 'exit 7', when it finishes, then goway exits 7 and the work dir is removed"
bound = true

[[acceptance]]
text = "Given a cargo command, when goway runs it, then CARGO_TARGET_DIR points to a free per-repo target slot"
bound = true

[[acceptance]]
text = "Given --report FILE, when the run ends, then FILE holds host, arch, address and exit code as JSON"
bound = true
+++
