+++
id = "01M41Q6D3QBJ6N93Q2HPG3XK9R"
title = "remote.ps1: the Windows side of goway's remote protocol, at parity with remote.sh"
type = "story"
category = "todo"
priority = "medium"
points = 8
parent = "01M41Q6CAC5EW88V3ZFSFJ1T6J"
reporter = "lognd"
created = "2026-10-03T20:27:24Z"
updated = "2026-10-03T20:27:24Z"
scope = ["crates/goway/src/remote.ps1", "crates/goway/src/remote.rs", "crates/goway/tests/**", ".github/workflows/ci.yml"]

[[acceptance]]
text = "Given remote.ps1, when the contract tests drive manifest, hashes, deletions, receive, envfile, run, probe, gc, doctor and purge, then they behave as remote.sh does (generation checks, labelled entries, marker-guarded purge, slots and locks), on Windows CI and under pwsh on Linux"
bound = false

[[acceptance]]
text = "Given a command line, when it reaches PowerShell, then every argument arrives exactly as given (no re-splitting, quotes and dollar signs preserved), proven by a round-trip test over hostile arguments"
bound = false

[[acceptance]]
text = "Given Ctrl-C or a dropped connection, when a run is in flight, then the Windows job tree is stopped and its slot released"
bound = false
+++
