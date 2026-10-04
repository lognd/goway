+++
id = "01M41T4HJEW3W92MXGMVHRF97S"
title = "goway-setup install --host --native: a Windows helper without WSL over OpenSSH, journaled"
type = "story"
category = "in-progress"
priority = "medium"
points = 5
parent = "01M41Q6CAC5EW88V3ZFSFJ1T6J"
reporter = "lognd"
created = "2026-10-03T21:18:48Z"
updated = "2026-10-04T00:30:55Z"
scope = ["crates/goway-setup/src/native.rs", "crates/goway-setup/src/cli.rs", "crates/goway-setup/src/host.rs", "crates/goway-setup/src/helper.rs", "crates/goway-journal/src/change.rs", "scripts/windows/roundtrip-host.sh", "docs/install-windows.md", "crates/goway-setup/tests/native.rs", "crates/goway-setup/src/lib.rs", "crates/goway-setup/src/hostsys.rs", "crates/goway-setup/src/ps.rs", "crates/goway-setup/src/sysapi.rs", "crates/goway-setup/src/render.rs", "crates/goway-setup/src/app.rs", "crates/goway-setup/tests/elevated.rs", "crates/goway-setup/tests/relay.rs", "crates/goway-setup/tests/host_plan.rs", "crates/goway-setup/src/error.rs"]

[[links]]
kind = "blocked-by"
target = "01M41Q6CX4RRFZT58EE995C0C6"

[[acceptance]]
text = "Given a Windows laptop, when goway-setup install --host --native runs, then it enables the OpenSSH Server capability (recorded so uninstall removes it only if it was absent), starts sshd on boot, opens the port to local networks only, sets PowerShell as the default shell, and prints the goway add line with the host key fingerprint"
bound = true

[[acceptance]]
text = '''Given the installing account is an administrator, when the key is installed, then it goes to ProgramData\ssh\administrators_authorized_keys with the required ACL (SYSTEM and Administrators only); otherwise to the user's authorized_keys with a user-only ACL; both journaled'''
bound = false

[[acceptance]]
text = "Given goway-setup uninstall --host --native, when it runs, then every change is reverted exactly (snapshot test on a real machine in a test profile, never touching an existing sshd setup the user made)"
bound = false
+++
