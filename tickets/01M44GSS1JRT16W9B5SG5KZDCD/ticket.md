+++
id = "01M44GSS1JRT16W9B5SG5KZDCD"
title = "goway doctor diagnoses an unreachable WSL helper from its Windows side and the manual explains the fix"
type = "story"
category = "in-progress"
priority = "high"
points = 3
reporter = "lognd"
created = "2026-10-04T22:33:22Z"
updated = "2026-10-04T23:18:01Z"
scope = ["crates/goway/src/doctor.rs", "crates/goway/src/doctor/wsl_down.rs", "crates/goway/tests/doctor_wsl_down.rs", "docs/troubleshooting.md"]

[[acceptance]]
text = "Given a helper whose WSL port is closed and whose Windows ssh answers with the distro Stopped, When goway doctor runs, Then it says the distro is stopped and prints the command that starts the keepalive"
bound = true

[[acceptance]]
text = "Given a keepalive task with only a boot trigger, When goway doctor runs, Then it warns that a WSL shutdown leaves the helper offline and points at the fix"
bound = true

[[acceptance]]
text = "Given an address that does not answer at all, When goway doctor runs, Then it says the machine is unreachable (not that WSL is stopped)"
bound = true

[[acceptance]]
text = "Given the manual, When a reader opens the debugging section, Then it lists the by-hand checks: ping, port probe, wsl -l -v over Windows ssh, schtasks /query, and how to restart"
bound = true
+++

When the WSL ssh port does not answer but the same address answers Windows OpenSSH (or another configured Windows transport), doctor should look through Windows: wsl.exe -l -v (distro Stopped/Running), the keepalive task (present, last run, last result, has a repeating trigger), and the boot time; then print the exact fix (schtasks /run the keepalive, or goway-setup install to replace a boot-only task). The manual's debugging section gets the same steps written out by hand.
