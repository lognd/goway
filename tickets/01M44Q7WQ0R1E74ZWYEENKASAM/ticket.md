+++
id = "01M44Q7WQ0R1E74ZWYEENKASAM"
title = "Doctor's Windows-side WSL probe can hang server-side and pile up stuck sshd sessions on the helper"
type = "bug"
category = "in-progress"
priority = "critical"
points = 2
reporter = "lognd"
created = "2026-10-05T00:25:56Z"
updated = "2026-10-05T00:27:09Z"
scope = ["crates/goway/src/doctor/wsl_down.rs", "crates/goway/tests/doctor_wsl_down.rs", "docs/troubleshooting.md"]

[[acceptance]]
text = "Given a Windows-side probe whose wsl.exe never returns, When doctor runs, Then the generated command kills it on the helper after a bounded time and doctor reports that the WSL service is not responding"
bound = false

[[acceptance]]
text = "Given the hung-service finding, When doctor prints it, Then it gives the recovery steps (stop stuck wsl and sshd processes, restart sshd, wsl --shutdown, wslservice as a last resort)"
bound = false

[[acceptance]]
text = "Given the generated probe scripts, When tested, Then every Windows-side command in the WSL-down check carries a server-side timeout"
bound = false
+++

Observed 2026-10-04: with a helper's WSL service deadlocked, read-only diagnostics over Windows OpenSSH (wsl.exe -l -v, tasklist, a PowerShell query) hung; the client gave up after 30-45 s but the server-side sessions and their children stayed, and sshd.exe sat at about 45% CPU while refusing new logins. The new WSL-down doctor check runs the same wsl.exe -l -v and would do the same. Every Windows-side probe must carry a server-side time limit (run it as a job with Wait-Job -Timeout and Stop-Job, or Start-Process with WaitForExit(ms) and Kill), so a hung wsl.exe is killed on the helper, and doctor reports a hung WSL service as its own finding with the recovery steps.
