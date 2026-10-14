+++
id = "01M4WB91QMTXFBW34MT9SC8EN4"
title = "install --host cannot finish on WSL 2.5+: set-sparse is refused, so the install always rolls back"
type = "bug"
category = "todo"
priority = "critical"
class = "expedite"
reporter = "lognd"
created = "2026-10-14T04:38:37Z"
updated = "2026-10-14T04:50:39Z"
scope = ["crates/goway-setup/src/hostsys.rs", "crates/goway-setup/src/host.rs", "crates/goway-setup/src/cli.rs", "crates/goway-setup/src/tune.rs", "docs/install-windows.md", "docs/usage.md"]

[[acceptance]]
text = "Without --allow-unsafe-sparse, the install --host plan contains no sparse virtual disk change, so the install finishes on a WSL that refuses set-sparse"
bound = false

[[acceptance]]
text = "With --allow-unsafe-sparse, install --host plans the sparse change and applies it as wsl --manage DISTRO --set-sparse true --allow-unsafe"
bound = false

[[acceptance]]
text = "tune --sparse --allow-unsafe-sparse passes --allow-unsafe to wsl; tune --sparse alone does not"
bound = false

[[acceptance]]
text = "A failed wsl command reports its stdout when stderr is empty, so WSL's own reason reaches the user"
bound = false
+++

Seen 2026-10-14 on Nova (Windows 11 26200 ARM64, WSL NAT, Ubuntu-22.04, goway-setup 0.1.1) run from inside WSL. All 23 earlier changes applied, then change 23 'ensure sparse WSL virtual disk' ran `wsl --manage Ubuntu-22.04 --set-sparse true`, which exited -1 with empty output because the distro was running. The whole install rolled back. The dry run reported it as 'will do' with no warning. Running the installer from a WSL terminal is common, so the normal path cannot finish without a `wsl --shutdown` that ends the user's sessions.
