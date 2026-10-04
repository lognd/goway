+++
id = "01M41Q6DFH1EXKZXZ4WYGM627C"
title = "Rust on Windows hosts: per-repository CARGO_TARGET_DIR, msvc target, warm reruns"
type = "story"
category = "todo"
priority = "medium"
points = 3
parent = "01M41Q6CAC5EW88V3ZFSFJ1T6J"
reporter = "lognd"
created = "2026-10-03T20:27:24Z"
updated = "2026-10-04T00:35:06Z"
scope = ["crates/goway/src/remote.ps1", "crates/goway/src/run.rs", "docs/usage.md"]

[[acceptance]]
text = "Given a Windows host, when goway run -- cargo nextest run --workspace runs twice, then both use a persistent per-repository target directory on the Windows side, the toolchain target is x86_64-pc-windows-msvc, and the second run is warm"
bound = false

[[acceptance]]
text = "Given a run, when it finishes, then goway reports host, os windows and architecture, and exits with the command's exit code"
bound = false
+++
