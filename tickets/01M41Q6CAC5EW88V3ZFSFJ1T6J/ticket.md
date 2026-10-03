+++
id = "01M41Q6CAC5EW88V3ZFSFJ1T6J"
title = "Native Windows hosts: run on the local Windows side through interop, or on Windows machines over OpenSSH"
type = "epic"
category = "todo"
priority = "medium"
points = 13
reporter = "lognd"
created = "2026-10-03T20:27:23Z"
updated = "2026-10-03T20:27:23Z"
scope = ["crates/**", "docs/**", ".github/**", "README.md"]

[[acceptance]]
text = "Given a WSL work tree, when goway run --host win -- cargo nextest run --workspace runs, then the tests run natively on Windows (x86_64-pc-windows-msvc) with live output, the real exit code, and the host and architecture reported"
bound = false
+++
