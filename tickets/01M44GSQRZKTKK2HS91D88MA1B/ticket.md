+++
id = "01M44GSQRZKTKK2HS91D88MA1B"
title = "The WSL keepalive task starts only at boot, so a WSL shutdown leaves the helper offline until reboot"
type = "bug"
category = "in-progress"
priority = "critical"
points = 2
reporter = "lognd"
created = "2026-10-04T22:33:20Z"
updated = "2026-10-04T22:52:44Z"
scope = ["crates/goway-setup/src/ps.rs", "crates/goway-setup/tests/host_plan.rs", "crates/goway-setup/src/hostsys.rs", "crates/goway-setup/src/cli.rs", "crates/goway-setup/tests/hostsys.rs", "crates/goway-journal/src/system.rs", "crates/goway-journal/src/plan.rs", "crates/goway-journal/src/journal.rs", "crates/goway-journal/src/model.rs", "crates/goway-setup/src/host.rs", "crates/goway-journal/tests/scenarios.rs", "docs/install-windows.md"]

[[acceptance]]
text = "Given a boot or logon keepalive, When the task script is generated, Then its trigger repeats every few minutes with MultipleInstances IgnoreNew so a running keepalive is not duplicated"
bound = true

[[acceptance]]
text = "Given a keepalive whose distro was shut down, When the next repetition fires, Then the distro is started again without user action (manual check documented in the done report)"
bound = false

[[acceptance]]
text = "Given an existing install with the old boot-only task, When goway-setup install or tune runs again, Then the task is replaced with the repeating one"
bound = true
+++

Observed: a helper's distro was Stopped while its keepalive task was Ready, last run hours earlier with result 0. The task has only an AtStartup/AtLogOn trigger; once anything shuts WSL down (wsl --shutdown, a Windows update, goway-setup tune), sleep infinity ends and nothing restarts it. The relay task already repeats every few minutes; the keepalive should too.
