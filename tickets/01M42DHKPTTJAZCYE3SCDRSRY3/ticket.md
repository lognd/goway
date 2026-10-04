+++
id = "01M42DHKPTTJAZCYE3SCDRSRY3"
title = "A helper that suspends or drops off mid-job is reported as asleep, not as a command failure"
type = "story"
category = "in-progress"
priority = "high"
points = 2
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-04T02:58:00Z"
updated = "2026-10-04T03:55:49Z"
scope = ["crates/goway/src/run.rs", "crates/goway/tests/host_asleep.rs", "docs/troubleshooting.md"]

[[acceptance]]
text = "Given a job whose ssh connection drops and whose host then no longer answers, when goway returns, then it reports the host as asleep, off or off the network with exit 125 and not as a failure of the command, while a command that itself exits 255 on a host that still answers keeps its exit code"
bound = true
+++

split from ~PSR953W by lane C; the patch is in the lane C scratchpad asleep-run.patch (host_answers probe in stream)
