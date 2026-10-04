+++
id = "01M4264NEAS0KYAAM0X3JJQ4YK"
title = "Windows host runs cost about 10s of overhead (one powershell.exe start per call); batch calls or keep one PowerShell session per run"
type = "task"
category = "in-progress"
priority = "medium"
points = 3
parent = "01M41Q6CAC5EW88V3ZFSFJ1T6J"
reporter = "lognd"
created = "2026-10-04T00:48:35Z"
updated = "2026-10-04T02:55:18Z"
scope = ["crates/goway/src/remote.ps1", "crates/goway/src/run.rs", "crates/goway/src/interop.rs", "crates/goway/src/transport.rs", "docs/design.md", "crates/goway/src/sync.rs", "crates/goway/src/pool.rs", "crates/goway/src/session.rs", "crates/goway/src/lib.rs", "crates/goway/src/remote.rs", "crates/goway/tests/ps_session.rs"]

[[acceptance]]
text = "Given a warm, unchanged repository on a Windows host, when goway run -- cmd /c exit 0 runs, then goway's own overhead (everything but the command) is at most 3 seconds on the owner's ARM laptop through interop, measured and recorded in the ticket, by batching probe, sync and verify into fewer powershell.exe starts or reusing one session per run"
bound = true
+++
