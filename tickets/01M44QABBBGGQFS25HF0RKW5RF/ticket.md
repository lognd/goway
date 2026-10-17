+++
id = "01M44QABBBGGQFS25HF0RKW5RF"
title = "Doctor's Windows facts probe runs rustup and vswhere with no server-side time limit"
type = "bug"
category = "in-progress"
priority = "medium"
points = 1
reporter = "lognd"
created = "2026-10-05T00:27:16Z"
updated = "2026-10-17T09:15:44Z"
scope = ["crates/goway/src/doctor/windows.rs", "crates/goway/tests/doctor_output.rs"]

[[acceptance]]
text = "Given rustup or vswhere hangs on a Windows helper, When doctor probes Windows facts, Then the probe is killed after a bounded time and doctor reports a probe warning"
bound = false
+++

found while working ENKASAM: FACTS_PS in doctor/windows.rs (and remote.ps1 callers) rely on the ssh client timeout only; wrap the external programs with the bounded_command pattern from doctor/wsl_down.rs
