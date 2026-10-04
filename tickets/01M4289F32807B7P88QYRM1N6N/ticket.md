+++
id = "01M4289F32807B7P88QYRM1N6N"
title = "Windows PowerShell 5.1 on CI: a slot with nothing changed rebuilds (stamp test still fails at the warm step)"
type = "bug"
category = "todo"
priority = "medium"
points = 3
reporter = "lognd"
created = "2026-10-04T01:26:10Z"
updated = "2026-10-04T01:26:10Z"
scope = ["crates/goway/tests/remote_ps1.rs", "crates/goway/src/remote.ps1"]

[[acceptance]]
text = "Given Windows PowerShell 5.1 on CI, when a second run has nothing changed, then the slot stays warm (written=0, no rebuild)"
bound = false
+++

CI run 37167653172 windows 5.1: remote_ps1.rs:766 left [rebuilt, from B] right [from B]. YKF9Q5D's test-only fix did not help; fix in the product.
