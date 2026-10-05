+++
id = "01M44S31BX2R53R7245KMJRY4R"
title = "remote.ps1 fails every verb: the bounded slot-wait block landed at the top of the file instead of in Verb-run"
type = "bug"
category = "in-progress"
priority = "critical"
points = 1
reporter = "lognd"
created = "2026-10-05T00:58:14Z"
updated = "2026-10-05T01:07:29Z"
scope = ["crates/goway/src/remote.ps1"]

[[acceptance]]
text = "Given remote.ps1, When the remote_ps1 contract tests and the slot_wait test run under pwsh, Then they pass, and Verb-run's busy-slot wait is the bounded one"
bound = true
+++

CI on main (Windows pwsh contract tests, Linux Windows-host tests) fails every remote.ps1 call with 'Write-Err is not recognized' from the trap at line 79: the slot-wait block from ~D2QQV2Q sits at lines 1-22, outside any function, and runs before anything is defined; Verb-run still has the old unbounded wait. Helpers used for verification have no pwsh, so the tests were skipped there.
