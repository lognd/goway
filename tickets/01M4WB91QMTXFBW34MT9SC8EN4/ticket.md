+++
id = "01M4WB91QMTXFBW34MT9SC8EN4"
title = "install --host cannot finish on WSL 2.5+: set-sparse is refused, so the install always rolls back"
type = "bug"
category = "done"
outcome = "done"
priority = "critical"
class = "expedite"
reporter = "lognd"
created = "2026-10-14T04:38:37Z"
updated = "2026-10-14T04:59:40Z"
scope = ["crates/goway-setup/src/hostsys.rs", "docs/install-windows.md", "docs/usage.md", "crates/goway-setup/tests/hostsys.rs", "crates/goway-setup/src/host.rs", "crates/goway-setup/tests/host_plan.rs"]

[[acceptance]]
text = "When wsl refuses set-sparse true because sparse VHDs are disabled and suggests --allow-unsafe, goway reruns it with --allow-unsafe and the change succeeds"
bound = true

[[acceptance]]
text = "On a WSL that accepts the plain command, goway runs set-sparse without --allow-unsafe"
bound = true

[[acceptance]]
text = "A failed wsl command reports its stdout when stderr is empty, so WSL's own reason reaches the user"
bound = true

[[acceptance]]
text = "The install --host plan has no sparse virtual disk change (WSL changes the disk only while the distro is stopped, and the install needs it running) and keeps the fstrim timer"
bound = true
+++

Seen 2026-10-14 on Nova (Windows 11 26200 ARM64, WSL NAT, Ubuntu-22.04, goway-setup 0.1.1) run from inside WSL. All 23 earlier changes applied, then change 23 'ensure sparse WSL virtual disk' ran `wsl --manage Ubuntu-22.04 --set-sparse true`, which exited -1 with empty output because the distro was running. The whole install rolled back. The dry run reported it as 'will do' with no warning. Running the installer from a WSL terminal is common, so the normal path cannot finish without a `wsl --shutdown` that ends the user's sessions.
