+++
id = "01M44F3BBCMAXQNP7H2SZFHCRG"
title = "A job in a systemd-run scope survives its vanished client: stop the whole scope (cgroup), not one process group"
type = "bug"
category = "todo"
priority = "critical"
points = 3
parent = "01M418GXXC6312Z4N151DYH3Y4"
reporter = "lognd"
created = "2026-10-04T22:03:38Z"
updated = "2026-10-04T22:03:38Z"
scope = ["crates/goway/src/remote.sh", "crates/goway/tests/client_loss.rs", "crates/goway/tests/mem_footprint.rs", "docs/design.md"]

[[acceptance]]
text = "Given a job running in its goway-<run>.scope (systemd user scope, used for memory accounting) whose client vanished, when the lifeline or watchdog stops it, then the whole scope is stopped (systemctl --user stop or kill of the cgroup), so every process of the job ends however it regrouped; found live on 2026-10-04: a stress loop of 600 processes in their own process group outlived its client by over an hour at load 620, holding a build slot"
bound = false

[[acceptance]]
text = "Given the same in a test (a job that starts background subshells in a new process group, then the client is SIGKILLed), when the bound passes, then no process of the job remains and the slot lock is free; also for jobs without a scope (fallback path)"
bound = false
+++
